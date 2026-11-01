"""Configurações de áudio."""
ID = 'a35-dnd-slider'
CATEGORY = 'drag'
PAGE = 'argo35/dnd_slider.html'
PROMPT = "In 'Configurações de áudio', set 'Volume' to 35 and 'Graves' to 80. Leave the other settings unchanged. Then click 'Aplicar'."

def check(state, root):
    """Accept only the requested complete state."""
    try:
        ok = isinstance(state, dict) and (set(state) == {'values', 'applied'} and state.get('applied') is True and set(state.get('values', {})) == set({'Volume': 60, 'Graves': 50, 'Médios': 50, 'Agudos': 50, 'Balanço': 50}) and all(type(v) is int for v in state['values'].values()) and 34 <= state['values']['Volume'] <= 36 and 79 <= state['values']['Graves'] <= 81 and all(state['values'][k] == 50 for k in ['Médios','Agudos','Balanço']))
        return bool(ok), 'Estado correto' if ok else 'Estado incompleto ou alteração adicional'
    except (KeyError, TypeError, ValueError, AttributeError):
        return False, 'Estado inválido'

WRONG = [{'values': {'Volume': 60, 'Graves': 50, 'Médios': 50, 'Agudos': 50, 'Balanço': 50}, 'applied': False}, {'values': {'Volume': 35, 'Graves': 50, 'Médios': 50, 'Agudos': 50, 'Balanço': 50}, 'applied': True}, {'values': {'Volume': 60, 'Graves': 80, 'Médios': 50, 'Agudos': 50, 'Balanço': 50}, 'applied': True}]
