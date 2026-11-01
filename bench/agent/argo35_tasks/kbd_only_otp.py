"""Task contract and exact state checker."""
ID = 'a35-kbd-only-otp'
CATEGORY = 'keyboard-only'
PAGE = 'argo35/kbd_only_otp.html'
PROMPT = "In 'Banco Horizonte', enter the code shown in the SMS panel and card PIN 7350. Then click 'Confirmar'."
EXPECTED = {'otp': '482913', 'pin': '7350'}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'otp': '482913', 'pin': ''}, {'otp': '482931', 'pin': '7350'}, {'otp': '482913', 'pin': '7530'}]
