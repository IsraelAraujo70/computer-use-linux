"""Delete only "Projeto Legado", confirming the native dialogs with its exact name."""
ID = "a35-native-dialogs"
CATEGORY = "dialog"
PAGE = "argo35/native_dialogs.html"
PROMPT = 'Delete only "Projeto Legado", confirming the native dialogs with its exact name.'
EXPECTED = {'projects': ['Portal do cliente', 'Projeto Legado 2', 'Aplicativo móvel']}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'projects': ['Portal do cliente', 'Projeto Legado', 'Projeto Legado 2', 'Aplicativo móvel']}, {'projects': ['Portal do cliente', 'Projeto Legado', 'Aplicativo móvel']}, {'projects': ['Portal do cliente', 'Aplicativo móvel']}]
