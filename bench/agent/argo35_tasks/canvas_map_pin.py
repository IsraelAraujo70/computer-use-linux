import math
ID='a35-canvas-map-pin'
CATEGORY='canvas'
PAGE='argo35/canvas_map_pin.html'
PROMPT="In Lojas • Encontre sua unidade, select 'Loja Centro'. Then click 'Definir como minha loja'."
WRONG=[{'my_store': 'store-1'}, {'my_store': None}, {'my_store': 'store-2'}]
def check(state, root):
    if not isinstance(state,dict): return False, 'invalid state'
    return (state == {'my_store': 'store-0'}, 'store must be Loja Centro')
