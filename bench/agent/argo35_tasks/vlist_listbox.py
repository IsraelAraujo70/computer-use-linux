"""Cidade de entrega."""
ID = 'a35-vlist-listbox'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_listbox.html'
PROMPT = "In 'Cidade de entrega', choose 'Vila Nova do Sul'. Then click 'Salvar'."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'city': 'Vila Nova do Sul'} else (False, 'Requested state does not match')
WRONG = [{'city': 'Vila Nova'}, {'city': 'Vila Nova do Sul de Minas'}, {'city': None}]
