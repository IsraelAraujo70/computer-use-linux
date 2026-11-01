"""Benchmark task suite: what the agent is asked to do and how success is checked.

Every checker reads real state (the JSON a page posted to server.py, or a file
the terminal wrote); none of them trusts the agent's own claim.
"""
import json
import re
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

Check = Callable[[dict, Path], tuple[bool, str]]


@dataclass(frozen=True)
class Task:
    id: str
    kind: str  # "web" (Chrome page) or "terminal" (Alacritty)
    page: str  # path under /apps/ for web tasks
    prompt: str
    check: Check
    # Optional per-episode preparation (files to upload, ...), run before the app opens.
    setup: Callable[[Path], None] | None = None


def norm(text) -> str:
    """Case- and whitespace-insensitive form used for free-text comparisons."""
    text = unicodedata.normalize("NFC", str(text or ""))
    return re.sub(r"\s+", " ", text).strip().casefold()


def digits(text) -> str:
    return re.sub(r"\D", "", str(text or ""))


def expect(pairs: list[tuple[str, object, object]]) -> tuple[bool, str]:
    bad = [f"{name}={got!r} (want {want!r})" for name, got, want in pairs if got != want]
    return (not bad, "; ".join(bad) or "ok")


def check_mail_reply(state: dict, _: Path):
    sent = state.get("sent") or []
    if len(sent) != 1:
        return False, f"{len(sent)} replies sent (want 1)"
    reply = sent[0]
    return expect(
        [
            ("in_reply_to", reply.get("in_reply_to"), "m7"),
            ("body", norm(reply.get("body")).rstrip("."), "recebido, obrigado"),
            ("archived", state.get("archived") or [], []),
        ]
    )


def check_mail_label(state: dict, _: Path):
    labels = state.get("labels") or {}
    financeiro = sorted(k for k, v in labels.items() if "Financeiro" in v)
    others = sorted(k for k, v in labels.items() if set(v) - {"Financeiro"})
    return expect(
        [
            ("financeiro", financeiro, ["l23", "l4"]),
            ("other_labels", others, []),
            ("archived", state.get("archived") or [], []),
        ]
    )


def check_calendar(state: dict, _: Path):
    events = state.get("events") or []
    if len(events) != 1:
        return False, f"{len(events)} events created (want 1): {events}"
    event = events[0]
    return expect(
        [
            ("title", norm(event.get("title")), "dentista"),
            ("date", event.get("date"), "2026-11-18"),
            ("start", event.get("start"), "14:30"),
        ]
    )


def check_signup(state: dict, _: Path):
    s = state.get("submitted")
    if not s:
        return False, "form not submitted"
    return expect(
        [
            ("name", norm(s.get("name")), "joana ribeiro"),
            ("email", norm(s.get("email")), "joana.ribeiro@example.com"),
            ("phone", digits(s.get("phone")), "31987654321"),
            ("state", s.get("state"), "Minas Gerais"),
            ("plan", s.get("plan"), "Pro"),
            ("terms", s.get("terms"), True),
        ]
    )


def check_settings(state: dict, _: Path):
    s = state.get("saved")
    if not s:
        return False, "settings not saved"
    return expect(
        [
            ("frequency", s.get("frequency"), "weekly"),
            ("browser_notifications", s.get("browser_notifications"), True),
            ("email_digest", s.get("email_digest"), True),
            ("language", s.get("language"), "pt-BR"),
            ("display_name", s.get("display_name"), "Israel"),
        ]
    )


# Filled from the generated page (see apps/contacts.html): the id of "Valéria Nunes".
CONTACT_TARGET = "c170"


def check_contacts(state: dict, _: Path):
    return expect([("starred", sorted(state.get("starred") or []), [CONTACT_TARGET])])


def check_billing(state: dict, _: Path):
    b = state.get("billing")
    if not b:
        return False, "address not confirmed"
    return expect(
        [
            ("cep", digits(b.get("cep")), "30130010"),
            ("street", norm(b.get("street")), "avenida afonso pena"),
            ("number", digits(b.get("number")), "1500"),
            ("city", norm(b.get("city")), "belo horizonte"),
            ("uf", b.get("uf"), "MG"),
        ]
    )


def check_order_support(state: dict, _: Path):
    t = state.get("ticket")
    if not t:
        return False, "ticket not submitted"
    ok, detail = expect(
        [
            ("order", digits(t.get("order")), "4821"),
            ("tracking", norm(t.get("tracking")).upper(), "BR482193XQ"),
            ("category", t.get("category"), "Entrega"),
        ]
    )
    if "não chegou" not in norm(t.get("message")) and "nao chegou" not in norm(t.get("message")):
        return False, (detail + "; " if not ok else "") + f"message={t.get('message')!r}"
    return ok, detail


# Filled from the generated page (see apps/invoices.html): Acme Ltda + Vencida.
ACME_OVERDUE = ["NF-1003", "NF-1016", "NF-1028"]


def check_invoices(state: dict, _: Path):
    return expect([("archived", sorted(state.get("archived") or []), sorted(ACME_OVERDUE))])


def check_terminal(_: dict, root: Path):
    path = root / "term" / "nota.txt"
    if not path.exists():
        return False, f"{path} missing"
    content = path.read_text(errors="replace")
    return expect([("content", content.strip(), "ola mundo 42")])


TASKS = [
    Task(
        "mail-reply",
        "web",
        "mail-reply.html",
        "In the mail app, open the email from Marina Costa about the rental contract "
        "('Contrato de locação') and reply to it with exactly this text: Recebido, obrigado. "
        "Then send the reply.",
        check_mail_reply,
    ),
    Task(
        "mail-label",
        "web",
        "mail-label.html",
        "In the mail app, apply the label 'Financeiro' to every email from 'Banco Aurora' "
        "and to no other email. Do not archive anything.",
        check_mail_label,
    ),
    Task(
        "calendar-event",
        "web",
        "calendar.html",
        "In the calendar app, create an event titled 'Dentista' on November 18, 2026, "
        "starting at 14:30, and save it.",
        check_calendar,
    ),
    Task(
        "form-signup",
        "web",
        "signup.html",
        "Fill in the sign-up form with: name Joana Ribeiro, e-mail joana.ribeiro@example.com, "
        "phone (31) 98765-4321, state Minas Gerais, plan Pro; accept the terms and create the account.",
        check_signup,
    ),
    Task(
        "settings-dialog",
        "web",
        "settings.html",
        "In this dashboard, open the settings through the 'Mais opções' menu. In the notification "
        "settings set the frequency to weekly ('Semanal') and turn on browser notifications "
        "('Notificações no navegador'). Leave everything else unchanged and save.",
        check_settings,
    ),
    Task(
        "contacts-infinite",
        "web",
        "contacts.html",
        "In the contacts list, star the contact 'Valéria Nunes'. Do not star anyone else.",
        check_contacts,
    ),
    Task(
        "iframe-billing",
        "web",
        "checkout.html",
        "On the checkout page, fill in the billing address: CEP 30130-010, street Avenida Afonso Pena, "
        "number 1500, city Belo Horizonte, state MG. Then confirm the address.",
        check_billing,
    ),
    Task(
        "order-support",
        "web",
        "orders.html",
        "Open a support ticket for order #4821 with category 'Entrega', the order's tracking code "
        "(shown on that order's page) and the message 'Pedido não chegou'. Submit it.",
        check_order_support,
    ),
    Task(
        "invoices-bulk",
        "web",
        "invoices.html",
        "Archive every invoice of client 'Acme Ltda' whose status is 'Vencida', and no other invoice.",
        check_invoices,
    ),
    Task(
        "terminal",
        "terminal",
        "",
        "In the open terminal window, use a shell command to create the file "
        "/tmp/agent-cu-bench/term/nota.txt whose only content is the line: ola mundo 42",
        check_terminal,
    ),
]

BY_ID = {task.id: task for task in TASKS}


def suite(name: str) -> list[Task]:
    """Tasks of a named suite: "easy" (this module), "hard" (tasks_hard.py) or "long" (tasks_long.py)."""
    if name == "easy":
        return TASKS
    if name == "hard":
        import tasks_hard

        return tasks_hard.HARD_TASKS
    if name == "long":
        import tasks_long

        return tasks_long.LONG_TASKS
    raise ValueError(f"unknown suite {name!r}")


def by_id(task_id: str) -> Task:
    if task_id in BY_ID:
        return BY_ID[task_id]
    import tasks_hard
    import tasks_long

    return {t.id: t for t in tasks_hard.HARD_TASKS + tasks_long.LONG_TASKS}[task_id]


def read_state(state_dir: Path, task_id: str) -> dict:
    path = state_dir / f"{task_id}.json"
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}
