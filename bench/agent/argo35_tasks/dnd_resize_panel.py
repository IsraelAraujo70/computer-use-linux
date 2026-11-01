"""Ambiente de desenvolvimento."""
ID = 'a35-dnd-resize-panel'
CATEGORY = 'drag'
PAGE = 'argo35/dnd_resize_panel.html'
PROMPT = "In 'Ambiente de desenvolvimento', make 'Explorador' 600 px wide and 'Terminal' 300 px tall."

def check(state, root):
    """Accept only the requested complete state."""
    try:
        ok = isinstance(state, dict) and (set(state)=={'width','height'} and type(state['width']) is int and type(state['height']) is int and abs(state['width']-600)<=10 and abs(state['height']-300)<=10)
        return bool(ok), 'Estado correto' if ok else 'Estado incompleto ou alteração adicional'
    except (KeyError, TypeError, ValueError, AttributeError):
        return False, 'Estado inválido'

WRONG = [{'width': 280, 'height': 180}, {'width': 600, 'height': 180}, {'width': 280, 'height': 300}]
