import math
ID='a35-canvas-draw-rect'
CATEGORY='canvas'
PAGE='argo35/canvas_draw_rect.html'
PROMPT="In Estúdio • Identidade visual, use 'Retângulo' to draw exactly one rectangle covering 'Área do logo'. Then click 'Salvar'."
WRONG=[{'shapes': []}, {'shapes': [{'type': 'ellipse', 'x': 320, 'y': 180, 'w': 420, 'h': 240}]}, {'shapes': [{'type': 'rect', 'x': 0, 'y': 0, 'w': 40, 'h': 20}]}]
def check(state, root):
    if not isinstance(state,dict): return False, 'invalid state'
    v=state.get('shapes')
    if not isinstance(v,list) or len(v)!=1 or not isinstance(v[0],dict): return False, 'draw one rectangle'
    s=v[0]
    if s.get('type')!='rect' or any(type(s.get(k)) not in (int,float) or not math.isfinite(s[k]) for k in ('x','y','w','h')): return False, 'invalid rectangle'
    x,y,w,h=(s[k] for k in ('x','y','w','h'))
    if w<=0 or h<=0: return False, 'invalid dimensions'
    intersection=max(0,min(x+w,740)-max(x,320))*max(0,min(y+h,420)-max(y,180))
    return (intersection/(w*h+420*240-intersection)>=.8, 'rectangle must cover target')
