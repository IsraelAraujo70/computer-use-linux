"""Registros de auditoria."""
ID = 'a35-vlist-10k'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_10k.html'
PROMPT = "In 'Registros de auditoria', approve only 'Registro #4000' using its 'Aprovar' button."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'approved': [4000]} else (False, 'Requested state does not match')
WRONG = [{'approved': [3999]}, {'approved': [4000, 4001]}, {'approved': []}]
