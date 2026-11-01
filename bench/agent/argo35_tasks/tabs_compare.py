"""Compare the three supplier tabs and add the supplier with the lowest total price including freight to the order."""
ID = "a35-tabs-compare"
CATEGORY = "multi-window"
PAGE = "argo35/tabs_compare.html"
PROMPT = "In 'Comparar fornecedores', compare price plus freight on the supplier pages opened by the links. Enter the cheapest supplier's code and click 'Adicionar ao pedido'."
EXPECTED = {'supplier_code': 'BOR-42'}
def check(state, root):
    return (True, "Requested state saved") if isinstance(state, dict) and state == EXPECTED else (False, "Incomplete or unexpected changes")
WRONG = [{'supplier_code': 'ATL-10'}, {'supplier_code': 'CED-73'}, {'supplier_code': ''}]
