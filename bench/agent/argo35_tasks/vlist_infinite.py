"""Publicações."""
ID = 'a35-vlist-infinite'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_infinite.html'
PROMPT = 'Like only post #237 by Marina Duarte.'
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'liked': [237]} else (False, 'Requested state does not match')
WRONG = [{'liked': None}, {'liked': []}, {'liked': [237], 'extra': True}]
