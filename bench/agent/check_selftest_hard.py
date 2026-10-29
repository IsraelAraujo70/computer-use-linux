#!/usr/bin/env python3
"""Feed selftest_hard.mjs's STATE lines into the hard checkers: each must pass, and each
must fail on an empty state (the untouched page) and on realistic wrong outcomes."""
import hashlib
import json
import sys
from pathlib import Path

import tasks_hard

HAPPY_KANBAN = {**tasks_hard.KANBAN_START, "k1": "done"}

# Plausible ways a bot gets each task wrong; every one must be rejected.
WRONG = {
    "accents-form": [
        {"saved": {"name": "Joao Conceicao Araujo", "city": "Sao Paulo", "notes": "Preco: 1.234,56 EUR - otimo! Uber facade  acao enfase"}},
        {"saved": {"name": "João Conceição Araújo", "city": "São Paulo", "notes": "Preço: 1.234,56 € — ótimo! Über façade ação ênfase"}},
        {"saved": {"name": "joão conceição araújo", "city": "São Paulo", "notes": "Preço: 1.234,56 € — ótimo! Über façade 😀 ação ênfase"}},
    ],
    "rich-compose": [
        {"sent": {"to": [], "subject": "Relatório de outubro", "body_text": "Olá, Ana!\nSegue o relatório de outubro."}},
        {"sent": {"to": ["ana.souza@example.com"], "subject": "Relatório de outubro", "body_text": "Olá, Ana! Segue o relatório de outubro."}},
        {"sent": {"to": ["ana.souza@example.com"], "subject": "Relatorio de outubro", "body_text": "Olá, Ana!\nSegue o relatório de outubro."}},
    ],
    "flaky-field": [
        {"paid": {"name": "MARIA S OLIVEIRA", "card_digits": "1111111111111111", "expiry": "12/28", "cvv": "123"}},
        {"paid": {"name": "MARIA S OLIVEIRA", "card_digits": "4111111111111111", "expiry": "11/28", "cvv": "123"}},
    ],
    "virtual-list": [{"archived": ["1376"]}, {"archived": ["1377", "1378"]}, {"archived": ["1001"]}],
    "modal-focus": [
        {"sent": {"name": "Carlos Mendes", "email": "carlos@example.com", "message": "Quero cancelar minha assinatura."},
         "newsletter": {"email": "carlos@example.com"}},
        {"newsletter": {"email": "carlos@example.com"}},
        {"sent": {"name": "Carlos Mendes", "email": "carlos@example.com", "message": "Quero cancelar minha"}},
    ],
    "slow-page": [{"exported": ["Maio"]}, {"exported": ["Março", "Abril"]}, {"exported": ["Janeiro"]}],
    "drag-drop": [
        {"columns": {**tasks_hard.KANBAN_START, "k3": "done"}},
        {"columns": {**tasks_hard.KANBAN_START, "k1": "doing"}},
        {"columns": {**HAPPY_KANBAN, "k4": "done"}},
    ],
    "file-upload": [
        {"uploaded": {"name": "comprovante-setembro.pdf", "size": len(tasks_hard.pdf_bytes("Setembro", "123,45")),
                      "sha256": hashlib.sha256(tasks_hard.pdf_bytes("Setembro", "123,45")).hexdigest()}},
        {"uploaded": {"name": "comprovante-outubro.pdf", "size": 0, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}},
    ],
    "multi-tab": [
        {"paid": {"code": "34191.79001 01043.510047 91020.150008 1 96610000012346"}},
        {"paid": {"code": "34191.79001 01043.510047 91020.150008 1 9661000001234"}},
    ],
    "xorigin-iframe": [
        {"transfer": {"key": "maria@example.com", "amount": "15000", "description": "Aluguel", "confirmed": True}},
        {"transfer": {"key": "maria@example.com", "amount": "150,00", "description": "Aluguel"}},
        {"transfer": {"key": "maria@exemple.com", "amount": "150,00", "description": "Aluguel", "confirmed": True}},
    ],
    "recover-error": [
        {"saved": {"name": "Israel Araujo", "email": "israel.araujo@example.com", "phone": "(11) 90000-0000(11) 91234-5678"}},
        {"saved": {"name": "", "email": "israel.araujo@example.com", "phone": "(11) 91234-5678"}},
        {"saved": {"name": "Israel Araujo", "email": "israel.araujo@example.com", "phone": "(11) 90000-0000"}},
    ],
    "async-autocomplete": [
        {"saved": {"cep": "88010-000", "street": "Rua Felipe Schmidt", "number": "200", "city": "Floriano", "uf": "PI"}},
        {"saved": {"cep": "88010-000", "street": "Rua Felipe Schmidt", "number": "200", "city": "Florianopolis", "uf": "SC"}},
    ],
}

nowhere = Path("/nonexistent")
by_id = {t.id: t for t in tasks_hard.HARD_TASKS}
bad = 0
seen = set()
for line in Path(sys.argv[1]).read_text().splitlines():
    if not line.startswith("STATE "):
        continue
    _, task_id, payload = line.split(" ", 2)
    seen.add(task_id)
    task = by_id[task_id]
    ok, detail = task.check(json.loads(payload), nowhere)
    empty_ok, _ = task.check({}, nowhere)
    wrong_ok = [w for w in WRONG[task_id] if task.check(w, nowhere)[0]]
    print(f"{task_id:19} happy={'PASS' if ok else 'FAIL ' + detail}  empty={'WRONGLY PASSES' if empty_ok else 'fails (good)'}"
          f"  wrong={len(WRONG[task_id]) - len(wrong_ok)}/{len(WRONG[task_id])} rejected")
    for w in wrong_ok:
        print("   wrongly accepted:", json.dumps(w, ensure_ascii=False))
    bad += (not ok) + empty_ok + len(wrong_ok)
missing = set(by_id) - seen
if missing:
    print("no selftest flow for:", sorted(missing))
    bad += 1
sys.exit(1 if bad else 0)
