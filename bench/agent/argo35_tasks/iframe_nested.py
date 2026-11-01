ID = "a35-iframe-nested"
CATEGORY = 'iframe'
PAGE = "argo35/iframe_nested.html"
PROMPT = 'Open a ticket for order 48213, reason "Produto com defeito", description "Tela trincada na entrega".'
TARGET = {'order': '48213', 'reason': 'Produto com defeito', 'description': 'Tela trincada na entrega'}
def check(state, root):
    return (isinstance(state, dict) and state == TARGET, "Expected exact requested state")
WRONG = [{'order': '48231', 'reason': 'Produto com defeito', 'description': 'Tela trincada na entrega'}, {'order': '48213', 'reason': 'Dúvida', 'description': 'Tela trincada na entrega'}, {'order': '48213', 'reason': 'Produto com defeito', 'description': ''}]
