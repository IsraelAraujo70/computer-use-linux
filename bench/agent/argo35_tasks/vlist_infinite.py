"""Publicações."""
ID = 'a35-vlist-infinite'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_infinite.html'
PROMPT = "In 'Publicações', find post #237 by Marina Duarte and click 'Curtir'. Leave other posts unchanged."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'liked': [237]} else (False, 'Requested state does not match')
WRONG = [{'liked': [236]}, {'liked': [237, 238]}, {'liked': []}]
