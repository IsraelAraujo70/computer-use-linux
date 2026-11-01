"""Prioridades da sprint."""
ID = 'a35-dnd-reorder'
CATEGORY = 'drag'
PAGE = 'argo35/dnd_reorder.html'
PROMPT = "In 'Prioridades da sprint', move 'Migrar banco de dados' to the top and 'Atualizar dependências' to the bottom. Keep all other tasks in relative order."

def check(state, root):
    """Accept only the requested complete state."""
    try:
        ok = isinstance(state, dict) and (state == {'order': ['2', '0', '1', '3', '5', '6', '7', '4']})
        return bool(ok), 'Estado correto' if ok else 'Estado incompleto ou alteração adicional'
    except (KeyError, TypeError, ValueError, AttributeError):
        return False, 'Estado inválido'

WRONG = [{'order': ['0', '1', '2', '3', '4', '5', '6', '7']}, {'order': ['2', '1', '0', '3', '4', '5', '6', '7']}, {'order': ['7', '6', '5', '4', '3', '2', '1', '0']}]
