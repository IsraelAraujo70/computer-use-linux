"""Logs da aplicação."""
ID = 'a35-vlist-logs'
CATEGORY = 'virtual-list'
PAGE = 'argo35/vlist_logs.html'
PROMPT = 'Mark only the log containing ERR-7731 as resolved.'
def check(state, root):
    return (True, 'Exact requested change') if isinstance(state, dict) and state == {'resolved': [6200]} else (False, 'Requested state does not match')
WRONG = [{'resolved': None}, {'resolved': []}, {'resolved': [6200], 'extra': True}]
