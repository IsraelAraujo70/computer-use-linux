"""Cidade de entrega."""
ID = 'a35-vlist-listbox'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_listbox.html'
PROMPT = "Choose 'Vila Nova do Sul' and click 'Salvar'."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'city': 'Vila Nova do Sul'} else (False, 'Requested state does not match')
WRONG = [{'city': None}, {'city': []}, {'city': 'Vila Nova do Sul', 'extra': True}]
