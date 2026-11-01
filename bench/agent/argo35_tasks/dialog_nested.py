"""Set "Fuso horário" to America/Manaus in "Configurações" and confirm. Do not accept cookies."""
ID = "a35-dialog-nested"
CATEGORY = "dialog"
PAGE = "argo35/dialog_nested.html"
PROMPT = 'Set "Fuso horário" to America/Manaus in "Configurações" and confirm. Do not accept cookies.'
EXPECTED = {'timezone': 'America/Manaus', 'cookies': 'recusado'}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and set(state) == {'timezone', 'cookies'} and state.get('timezone') == 'America/Manaus' and state.get('cookies') in (None, 'recusado') else (False, "Incomplete or unexpected changes")
WRONG = [{'timezone': 'America/Sao_Paulo', 'cookies': 'recusado'}, {'timezone': 'America/Manaus', 'cookies': 'aceito'}, {'timezone': 'America/Sao_Paulo', 'cookies': None}]
