"""Delete only "Projeto Legado", confirming the native dialogs with its exact name."""
ID = "a35-native-dialogs"
CATEGORY = "dialog"
PAGE = "argo35/native_dialogs.html"
PROMPT = "In 'Projetos', delete only 'Projeto Legado', confirming the native dialogs with its exact name."
EXPECTED = {'projects': ['Portal do cliente', 'Migração fiscal', 'Site institucional', 'Central de ajuda', 'Projeto Legado 2', 'Aplicativo móvel', 'Integração ERP', 'Painel financeiro', 'Catálogo digital', 'Onboarding', 'Auditoria']}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'projects': ['Portal do cliente', 'Projeto Legado', 'Projeto Legado 2', 'Aplicativo móvel']}, {'projects': ['Portal do cliente', 'Projeto Legado', 'Aplicativo móvel']}, {'projects': ['Portal do cliente', 'Aplicativo móvel']}]
