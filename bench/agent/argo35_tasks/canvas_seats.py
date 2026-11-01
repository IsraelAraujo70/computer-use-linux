import math
ID='a35-canvas-seats'
CATEGORY='canvas'
PAGE='argo35/canvas_seats.html'
PROMPT="In Cinema • Reserva de assentos, choose F7 and F8. Then click 'Confirmar assentos'."
WRONG=[{'seats': ['F7']}, {'seats': ['F7', 'F9']}, {'seats': ['F7', 'F8', 'F9']}]
def check(state, root):
    if not isinstance(state,dict): return False, 'invalid state'
    v = state.get('seats'); return (isinstance(v, list) and len(v) == 2 and all(isinstance(x,str) for x in v) and set(v) == {'F7','F8'}, 'select exactly F7 and F8')
