"""Registros de auditoria."""
ID = 'a35-vlist-10k'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_10k.html'
PROMPT = "Approve only 'Registro #4000'."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'approved': [4000]} else (False, 'Requested state does not match')
WRONG = [{'approved': None}, {'approved': []}, {'approved': [4000], 'extra': True}]
