"""Task contract and exact state checker."""
ID = 'a35-kbd-palette'
CATEGORY = 'shortcut'
PAGE = 'argo35/kbd_palette.html'
PROMPT = "In 'Aurora Code', enable 'Ativar quebra de linha automática', choose 'Solarizado claro', and set 'Indentação: 2 espaços'. Leave other settings unchanged."
EXPECTED = {'wrap': True, 'theme': 'Solarizado claro', 'indent': 2, 'minimap': True}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'wrap': False, 'theme': 'Solarizado claro', 'indent': 2, 'minimap': True}, {'wrap': True, 'theme': 'Escuro', 'indent': 2, 'minimap': True}, {'wrap': True, 'theme': 'Solarizado claro', 'indent': 2, 'minimap': False}]
