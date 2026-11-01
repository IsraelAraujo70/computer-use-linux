"""Task contract and exact state checker."""
ID = 'a35-ctx-submenu'
CATEGORY = 'context-menu'
PAGE = 'argo35/ctx_submenu.html'
PROMPT = 'Move the message from "Fornecedor XPTO" with subject "Cotação de servidores" to Projetos > Projeto Aurora. Leave other messages unchanged.'
EXPECTED = {'folders': {'m1': 'Projetos/Projeto Aurora', 'm2': 'Entrada', 'm3': 'Entrada', 'm4': 'Entrada'}}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'folders': {'m1': 'Entrada', 'm2': 'Entrada', 'm3': 'Entrada', 'm4': 'Entrada'}}, {'folders': {'m1': 'Projetos/Projeto Boreal', 'm2': 'Entrada', 'm3': 'Entrada', 'm4': 'Entrada'}}, {'folders': {'m1': 'Entrada', 'm2': 'Projetos/Projeto Aurora', 'm3': 'Entrada', 'm4': 'Entrada'}}]
