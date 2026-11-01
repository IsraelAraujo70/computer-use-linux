import math
ID='a35-canvas-connect'
CATEGORY='canvas'
PAGE='argo35/canvas_connect.html'
PROMPT="In Quadro • Fluxo de pedidos, use 'Conector' to connect 'Pedido recebido' to 'Pagamento aprovado' and 'Pagamento aprovado' to 'Enviar pedido'. Create no other connections."
WRONG=[{'edges': []}, {'edges': [[0, 1]]}, {'edges': [[0, 1], [1, 3]]}]
def check(state, root):
    if not isinstance(state,dict): return False, 'invalid state'
    v = state.get('edges'); return (isinstance(v,list) and len(v)==2 and all(isinstance(e,list) and len(e)==2 and all(type(x) is int for x in e) for e in v) and {tuple(e) for e in v} == {(0,1),(1,2)}, 'connect exactly the two requested edges')
