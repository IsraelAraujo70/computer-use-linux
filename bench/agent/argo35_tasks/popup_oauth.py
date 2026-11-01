"""Connect Google Drive with israel@empresa.com and click "Salvar integrações"."""
ID = "a35-popup-oauth"
CATEGORY = "multi-window"
PAGE = "argo35/popup_oauth.html"
PROMPT = "In the 'Integrações' page, connect Google Drive with israel@empresa.com. Then click 'Salvar integrações'."
EXPECTED = {'drive': {'connected': True, 'account': 'israel@empresa.com'}}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'drive': {'connected': False, 'account': None}}, {'drive': {'connected': True, 'account': 'israel.pessoal@gmail.com'}}, {'drive': {'connected': True, 'account': ''}}]
