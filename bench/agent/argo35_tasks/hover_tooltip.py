"""Subscribe to the plan whose support includes 24-hour phone support. Inspect the support explanations by hovering the information icons."""
ID = "a35-hover-tooltip"
CATEGORY = "hover"
PAGE = "argo35/hover_tooltip.html"
PROMPT = "In 'Planos para sua equipe', subscribe to the plan whose support includes 24-hour phone support by clicking 'Assinar'."
EXPECTED = {'plan': 'business'}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'plan': 'start'}, {'plan': 'pro'}, {'plan': 'enterprise'}]
