"""Set "Fuso horário" to America/Manaus in "Configurações" and confirm. Do not accept cookies."""
ID = "a35-dialog-nested"
CATEGORY = "dialog"
PAGE = "argo35/dialog_nested.html"
PROMPT = "In the team agenda, open 'Configurações', set 'Fuso horário' to 'America/Manaus' and click 'Salvar', then 'Confirmar'. Leave the other settings unchanged and do not accept cookies."
EXPECTED = {'timezone': 'America/Manaus', 'cookies': 'recusado', 'language': 'Português', 'dateFormat': 'DD/MM/AAAA', 'notifications': 'Ativadas'}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'timezone': 'America/Sao_Paulo', 'cookies': 'recusado'}, {'timezone': 'America/Manaus', 'cookies': 'aceito'}, {'timezone': 'America/Sao_Paulo', 'cookies': None}]
