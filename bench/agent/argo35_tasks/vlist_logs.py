"""Logs da aplicação."""
ID = 'a35-vlist-logs'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_logs.html'
PROMPT = "In 'Logs da aplicação', find the log containing 'ERR-7731' and click its 'Marcar como resolvido' button. Leave all other logs unchanged."
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'resolved': [6200]} else (False, 'Requested state does not match')
WRONG = [{'resolved': [6198]}, {'resolved': [6200, 6202]}, {'resolved': []}]
