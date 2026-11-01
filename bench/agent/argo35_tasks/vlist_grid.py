"""Inventário."""
ID = 'a35-vlist-grid'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_grid.html'
PROMPT = "In 'Inventário', set 'Estoque' of 'SKU-03120' to 42 and press Enter to commit. Change nothing else."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'edits': {'SKU-03120:Estoque': '42'}} else (False, 'Requested state does not match')
WRONG = [{'edits': {'SKU-03120:Estoque': '41'}}, {'edits': {'SKU-03120:Estoque': '42', 'SKU-03121:Estoque': '42'}}, {'edits': {}}]
