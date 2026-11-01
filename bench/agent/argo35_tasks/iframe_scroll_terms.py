ID = "a35-iframe-scroll-terms"
CATEGORY = 'iframe'
PAGE = "argo35/iframe_scroll_terms.html"
PROMPT = "In Aurora account signup, read the terms, select 'Li e aceito os termos', then click 'Continuar'."
TARGET = {'accepted': True, 'scrolled_to_end': True}
def check(state, root):
    return (isinstance(state, dict) and state == TARGET, "Expected exact requested state")
WRONG = [{'accepted': False, 'scrolled_to_end': True}, {'accepted': True, 'scrolled_to_end': False}, {'accepted': False, 'scrolled_to_end': False}]
