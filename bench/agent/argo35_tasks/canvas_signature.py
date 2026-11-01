"""Independent canvas_signature task contract."""
ID = "a35-canvas-signature"
CATEGORY = 'canvas'
PAGE = "argo35/canvas_signature.html"
PROMPT = "Sign in the 'Assinatura' box with a full signature, agree to 'Li e concordo com os termos', then click 'Assinar contrato'."
import math

def check(state, root):
    if not isinstance(state, dict): return False, 'invalid state'
    numbers = [state.get('stroke_px'), state.get('span_x')]
    ok = all(type(x) in (int, float) and math.isfinite(x) for x in numbers)
    return (bool(ok and state.get('signed') is True and state.get('agreed') is True and numbers[0] >= 250 and numbers[1] >= 200), 'signature and agreement required')
WRONG = [dict(signed=True, agreed=False, stroke_px=400, span_x=300), dict(signed=True, agreed=True, stroke_px=10, span_x=5), dict(signed=False, agreed=True, stroke_px=400, span_x=300)]
