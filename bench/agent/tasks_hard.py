"""Hard task suite: the things that break bots on real sites.

Pre-registered: these checkers were committed before any agent ran on the suite. Each one
accepts every correct way to finish its task and rejects wrong or incomplete outcomes.
Pages live under apps/hard/; see selftest_hard.mjs for a proven happy path per task.
"""
import hashlib
import re
import unicodedata
from decimal import Decimal, InvalidOperation
from pathlib import Path

from tasks import Task, digits, expect, norm


def exact(text) -> str:
    """Trimmed NFC form: no case or accent folding."""
    return unicodedata.normalize("NFC", str(text or "")).strip()


ACCENTS = {
    "name": "João Conceição Araújo",
    "city": "São Paulo",
    "notes": "Preço: 1.234,56 € — ótimo! Über façade 😀 ação ênfase",
}


def check_accents(state: dict, _: Path):
    s = state.get("saved")
    if not s:
        return False, "form not saved"
    return expect([(k, exact(s.get(k)), want) for k, want in ACCENTS.items()])


def check_compose(state: dict, _: Path):
    s = state.get("sent")
    if not s:
        return False, "message not sent"
    lines = [exact(line) for line in str(s.get("body_text") or "").splitlines()]
    return expect(
        [
            ("to", [norm(t) for t in s.get("to") or []], ["ana.souza@example.com"]),
            ("subject", exact(s.get("subject")), "Relatório de outubro"),
            ("body_lines", [line for line in lines if line], ["Olá, Ana!", "Segue o relatório de outubro."]),
        ]
    )


def check_payment(state: dict, _: Path):
    p = state.get("paid")
    if not p:
        return False, "not paid"
    return expect(
        [
            ("name", norm(p.get("name")), "maria s oliveira"),
            ("card_digits", digits(p.get("card_digits")), "4111111111111111"),
            ("expiry", exact(p.get("expiry")), "12/28"),
            ("cvv", exact(p.get("cvv")), "123"),
        ]
    )


def check_virtual_list(state: dict, _: Path):
    return expect([("archived", state.get("archived") or [], ["1377"])])


def check_modal(state: dict, _: Path):
    s = state.get("sent")
    if not s:
        return False, "message not sent" + ("; subscribed to the newsletter" if "newsletter" in state else "")
    return expect(
        [
            ("name", exact(s.get("name")), "Carlos Mendes"),
            ("email", exact(s.get("email")).casefold(), "carlos@example.com"),
            ("message", exact(s.get("message")), "Quero cancelar minha assinatura."),
            ("newsletter", state.get("newsletter"), None),
        ]
    )


def check_slow_page(state: dict, _: Path):
    return expect([("exported", state.get("exported") or [], ["Março"])])


KANBAN_START = {
    "k1": "todo", "k2": "todo", "k3": "todo",
    "k4": "doing", "k5": "doing", "k6": "doing",
    "k7": "done", "k8": "done",
}  # k1 = "Revisar contrato" (see apps/hard/kanban.html)


def check_drag_drop(state: dict, _: Path):
    cols = state.get("columns")
    if not cols:
        return False, "no card moved"
    return expect([("columns", cols, {**KANBAN_START, "k1": "done"})])


def pdf_bytes(month: str, amount: str) -> bytes:
    """A small, well-formed one-page PDF receipt; deterministic for given arguments."""
    lines = [f"Comprovante de pagamento - {month} de 2026", "Condominio Residencial Solar",
             "Pagador: Israel Araujo - Apto 302, Bloco B", f"Valor pago: R$ {amount}"]
    lines += [f"Autenticacao {i:02d}: {hashlib.sha256(f'{month}{i}'.encode()).hexdigest()[:48].upper()}" for i in range(10)]
    text = "".join(f"BT /F1 10 Tf 56 {780 - 26 * i} Td ({line}) Tj ET\n" for i, line in enumerate(lines))
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n%sendstream" % (len(text), text.encode()),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for n, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n%s\nendobj\n" % (n, body)
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return bytes(out)


UPLOAD_NAME = "comprovante-outubro.pdf"
UPLOAD_BYTES = pdf_bytes("Outubro", "123,45")


def setup_upload(root: Path) -> None:
    files = root / "files"
    files.mkdir(parents=True, exist_ok=True)
    (files / UPLOAD_NAME).write_bytes(UPLOAD_BYTES)
    (files / "comprovante-setembro.pdf").write_bytes(pdf_bytes("Setembro", "123,45"))
    jpeg = bytes.fromhex("ffd8ffe000104a46494600010100000100010000") + hashlib.sha256(b"foto").digest() * 40 + b"\xff\xd9"
    (files / "foto.jpg").write_bytes(jpeg)


def check_upload(state: dict, _: Path):
    u = state.get("uploaded")
    if not u:
        return False, "nothing uploaded"
    return expect(
        [
            ("name", u.get("name"), UPLOAD_NAME),
            ("size", u.get("size"), len(UPLOAD_BYTES)),
            ("sha256", u.get("sha256"), hashlib.sha256(UPLOAD_BYTES).hexdigest()),
        ]
    )


BOLETO_LINE = "34191.79001 01043.510047 91020.150008 1 96610000012345"


def check_multi_tab(state: dict, _: Path):
    p = state.get("paid")
    if not p:
        return False, "boleto not paid"
    return expect([("code", digits(p.get("code")), digits(BOLETO_LINE))])


def parse_brl(text) -> Decimal | None:
    """'150,00', '150', '150.00', 'R$ 1.500,00' -> Decimal; None when unparseable."""
    t = re.sub(r"R\$|\s", "", str(text or ""))
    if "," in t:
        t = t.replace(".", "").replace(",", ".")
    elif re.fullmatch(r"\d{1,3}(\.\d{3})+", t):
        t = t.replace(".", "")
    try:
        return Decimal(t)
    except InvalidOperation:
        return None


def check_xorigin(state: dict, _: Path):
    t = state.get("transfer")
    if not t:
        return False, "transfer not confirmed"
    return expect(
        [
            ("key", exact(t.get("key")).casefold(), "maria@example.com"),
            ("amount", parse_brl(t.get("amount")), Decimal("150")),
            ("description", exact(t.get("description")), "Aluguel"),
            ("confirmed", t.get("confirmed"), True),
        ]
    )


def check_recover(state: dict, _: Path):
    s = state.get("saved")
    if not s:
        return False, "profile not saved"
    return expect(
        [
            ("phone", digits(s.get("phone")), "11912345678"),
            ("name", exact(s.get("name")), "Israel Araujo"),
            ("email", exact(s.get("email")).casefold(), "israel.araujo@example.com"),
        ]
    )


def check_autocomplete(state: dict, _: Path):
    s = state.get("saved")
    if not s:
        return False, "address not saved"
    return expect(
        [
            ("cep", digits(s.get("cep")), "88010000"),
            ("street", norm(s.get("street")), "rua felipe schmidt"),
            ("number", digits(s.get("number")), "200"),
            ("city", s.get("city"), "Florianópolis"),
            ("uf", s.get("uf"), "SC"),
        ]
    )


HARD_TASKS = [
    Task(
        "accents-form",
        "web",
        "hard/accents.html",
        "In the 'Cadastro de cliente' form, fill in 'Nome completo' with \"João Conceição Araújo\", "
        "'Cidade' with \"São Paulo\" and 'Observações' with \"Preço: 1.234,56 € — ótimo! Über façade 😀 ação ênfase\" "
        "(each value without the surrounding double quotes). Keep every accent and symbol exactly as given, "
        "then click 'Salvar'.",
        check_accents,
    ),
    Task(
        "rich-compose",
        "web",
        "hard/compose.html",
        "In the 'Nova mensagem' window of the mail app, send an e-mail to ana.souza@example.com "
        "(field 'Para') with the subject ('Assunto') 'Relatório de outubro' and a body ('Corpo da mensagem') "
        "of exactly two lines: first line 'Olá, Ana!' and second line 'Segue o relatório de outubro.' "
        "Then click 'Enviar'.",
        check_compose,
    ),
    Task(
        "flaky-field",
        "web",
        "hard/payment.html",
        "On the 'Pagamento' page, pay with this card: 'Nome no cartão' MARIA S OLIVEIRA, 'Número do cartão' "
        "4111 1111 1111 1111, 'Validade' 12/28, 'CVV' 123. Then click 'Pagar'.",
        check_payment,
    ),
    Task(
        "virtual-list",
        "web",
        "hard/orders-virtual.html",
        "In the orders list ('Pedidos'), archive order #1377 using its 'Arquivar pedido #1377' button. "
        "Do not archive any other order.",
        check_virtual_list,
    ),
    Task(
        "modal-focus",
        "web",
        "hard/contact.html",
        "On the 'Fale conosco' page, send a message with 'Nome' Carlos Mendes, 'E-mail' carlos@example.com "
        "and 'Mensagem' 'Quero cancelar minha assinatura.' using the 'Enviar mensagem' button. "
        "Do not subscribe to any newsletter or offer.",
        check_modal,
    ),
    Task(
        "slow-page",
        "web",
        "hard/report.html",
        "On the 'Relatórios' page, generate the report and export the report for March ('Março') only.",
        check_slow_page,
    ),
    Task(
        "drag-drop",
        "web",
        "hard/kanban.html",
        "On the board 'Quadro - Projeto Aurora', move the card 'Revisar contrato' from the 'A fazer' column "
        "to the 'Concluído' column. Leave every other card where it is.",
        check_drag_drop,
    ),
    Task(
        "file-upload",
        "web",
        "hard/upload.html",
        "On the 'Enviar comprovante' page, choose the file /tmp/agent-cu-bench/files/comprovante-outubro.pdf "
        "in the 'Comprovante' field and click 'Enviar'.",
        check_upload,
        setup=setup_upload,
    ),
    Task(
        "multi-tab",
        "web",
        "hard/boleto.html",
        "Pay the pending boleto on the 'Pagar boleto' page: open it with the 'Ver boleto' link (it opens in a "
        "new tab), copy its 'Linha digitável' into the 'Linha digitável' field of the payment form, and click 'Pagar'.",
        check_multi_tab,
    ),
    Task(
        "xorigin-iframe",
        "web",
        "hard/bank.html",
        "In the Internet Banking 'Transferência Pix' form, make a Pix transfer with 'Chave Pix' maria@example.com, "
        "'Valor' 150,00 and 'Descrição' Aluguel. Click 'Continuar' and then 'Confirmar transferência'.",
        check_xorigin,
    ),
    Task(
        "recover-error",
        "web",
        "hard/profile.html",
        "On the 'Meu perfil' page, change 'Telefone' to (11) 91234-5678 and save it with 'Salvar alterações'. "
        "Leave the other fields unchanged. Make sure the change is actually saved.",
        check_recover,
    ),
    Task(
        "async-autocomplete",
        "web",
        "hard/shipping.html",
        "Fill in the 'Endereço de entrega' form: 'CEP' 88010-000, 'Rua' Rua Felipe Schmidt, 'Número' 200, "
        "'Cidade' Florianópolis (state SC). Then click 'Salvar endereço'.",
        check_autocomplete,
    ),
]
