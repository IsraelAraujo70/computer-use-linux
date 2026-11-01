"""Task contract and exact state checker."""
ID = 'a35-kbd-gmail'
CATEGORY = 'shortcut'
PAGE = 'argo35/kbd_gmail.html'
PROMPT = 'Archive all four messages labelled "Promoções" and star the message from "Banco Central". Change nothing else.'
EXPECTED = {'archived': ['m2', 'm5', 'm9', 'm13'], 'starred': ['m7']}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'archived': ['m2', 'm5', 'm9'], 'starred': ['m7']}, {'archived': ['m2', 'm5', 'm9', 'm13'], 'starred': []}, {'archived': ['m2', 'm5', 'm9', 'm13', 'm1'], 'starred': ['m7']}]
