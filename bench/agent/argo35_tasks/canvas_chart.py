import math
ID='a35-canvas-chart'
CATEGORY='canvas'
PAGE='argo35/canvas_chart.html'
PROMPT="In Vendas • Visão anual, select the month with the highest sales. Then click 'Abrir relatório'."
WRONG=[{'report_month': 'Ago'}, {'report_month': None}, {'report_month': 'Out'}]
def check(state, root):
    if not isinstance(state,dict): return False, 'invalid state'
    return (state == {'report_month':'Set'}, 'open September report')
