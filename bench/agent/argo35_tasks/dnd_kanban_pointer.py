"""Quadro da equipe."""
ID = 'a35-dnd-kanban-pointer'
CATEGORY = 'drag'
PAGE = 'argo35/dnd_kanban_pointer.html'
PROMPT = "In 'Quadro da equipe', move 'Corrigir login SSO' to the top of 'Concluído' and 'Escrever testes E2E' to 'Em andamento' at any position. Leave all other cards in their current column and relative order."

def check(state, root):
    """Accept only the requested complete state."""
    try:
        ok = isinstance(state, dict) and (set(state)=={'columns'} and set(state['columns'])=={'todo','doing','review','done'} and state['columns']['todo']==['design','api'] and state['columns']['review']==['docs','pagamento'] and state['columns']['done']==['sso','home','menu','tema'] and isinstance(state['columns']['doing'],list) and state['columns']['doing'].count('e2e')==1 and [x for x in state['columns']['doing'] if x!='e2e']==['busca','perfil','email'])
        return bool(ok), 'Estado correto' if ok else 'Estado incompleto ou alteração adicional'
    except (KeyError, TypeError, ValueError, AttributeError):
        return False, 'Estado inválido'

WRONG = [{'columns': {'todo': ['e2e', 'design', 'api'], 'doing': ['busca', 'perfil', 'email'], 'review': ['docs', 'sso', 'pagamento'], 'done': ['home', 'menu', 'tema']}}, {'columns': {'todo': ['e2e', 'design', 'api'], 'doing': ['busca', 'perfil', 'email'], 'review': ['docs', 'pagamento'], 'done': ['sso', 'home', 'menu', 'tema']}}, {'columns': {'todo': ['design', 'api'], 'doing': ['busca', 'perfil', 'email', 'e2e'], 'review': ['docs', 'sso', 'pagamento'], 'done': ['home', 'menu', 'tema']}}]
