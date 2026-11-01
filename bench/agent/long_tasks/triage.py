"""Exact mailbox outcome for the long inbox task."""
from copy import deepcopy
from tasks import expect

ID = "long-triage"
PAGE = "long/triage.html"
PROMPT = "Triage the inbox: apply the label 'Financeiro' to every email from 'Banco Aurora'; apply the label 'Clientes' to every email whose subject starts with 'Pedido #'; archive every email from 'Loja Vértice'; star every email from 'Marina Costa'. Do not change any other email."


def check(state, root):
    """Compare every message while ignoring read status."""
    messages = state.get("messages")
    if not isinstance(messages, dict) or set(messages) != {f"m{i}" for i in range(1, 25)}:
        return False, "messages must contain exactly m1 through m24"
    pairs = []
    for i in range(1, 25):
        message = messages[f"m{i}"]
        if not isinstance(message, dict):
            return False, f"m{i} must be an object"
        labels = ["Financeiro"] if i <= 4 else ["Clientes"] if i <= 8 else []
        actual_labels = message.get("labels")
        if not isinstance(actual_labels, list) or any(not isinstance(label, str) for label in actual_labels):
            return False, f"m{i} labels must be a list of strings"
        pairs.extend([
            (f"m{i} labels", sorted(actual_labels), labels),
            (f"m{i} archived", message.get("archived") is (9 <= i <= 13), True),
            (f"m{i} starred", message.get("starred") is (14 <= i <= 16), True),
        ])
    return expect(pairs)


WRONG = []
for identifier, field, value in [
    ("m17", "labels", ["Financeiro"]),
    ("m19", "starred", True),
    ("m18", "labels", ["Clientes"]),
    ("m13", "archived", False),
]:
    state = {"messages": {
        f"m{i}": {"labels": ["Financeiro"] if i <= 4 else ["Clientes"] if i <= 8 else [],
                  "archived": 9 <= i <= 13, "starred": 14 <= i <= 16, "read": False}
        for i in range(1, 25)
    }}
    state["messages"][identifier][field] = deepcopy(value)
    WRONG.append(state)
