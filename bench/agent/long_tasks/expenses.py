from collections import Counter
from copy import deepcopy
from tasks import norm

ID = 'long-expenses'
PAGE = 'long/expenses.html'
PROMPT = "Create an expense report titled 'Visita cliente Recife - outubro' for cost center Comercial and project Aurora with these five expenses, then send it for approval: 06/10/2026 Transporte 'Táxi aeroporto' 87,50; 06/10/2026 Hospedagem 'Hotel Boa Viagem 2 diárias' 640,00; 07/10/2026 Alimentação 'Almoço com cliente' 182,30; 07/10/2026 Transporte 'Uber reunião' 34,90; 08/10/2026 Material de escritório 'Impressões proposta' 45,00."

_EXPECTED = [['06/10/2026', 'Transporte', 'Táxi aeroporto', 8750], ['06/10/2026', 'Hospedagem', 'Hotel Boa Viagem 2 diárias', 64000], ['07/10/2026', 'Alimentação', 'Almoço com cliente', 18230], ['07/10/2026', 'Transporte', 'Uber reunião', 3490], ['08/10/2026', 'Material de escritório', 'Impressões proposta', 4500]]

def check(state, root):
    report = state.get('report')
    if not isinstance(report, dict): return False, 'report ausente'
    if norm(str(report.get('title', ''))) != norm('Visita cliente Recife - outubro'): return False, 'title incorreto'
    if report.get('cost_center') != 'Comercial': return False, 'cost_center incorreto'
    if report.get('project') != 'Aurora': return False, 'project incorreto'
    expenses = report.get('expenses')
    if not isinstance(expenses, list) or len(expenses) != 5: return False, 'expenses deve conter cinco itens'
    actual = []
    for e in expenses:
        if not isinstance(e, dict) or type(e.get('amount_cents')) is not int: return False, 'despesa inválida'
        if not all(isinstance(e.get(k), str) for k in ('date', 'category', 'description')): return False, 'campos de despesa inválidos'
        actual.append((e['date'], e['category'], norm(e['description']), e['amount_cents']))
    if Counter(actual) != Counter((d,c,norm(s),a) for d,c,s,a in _EXPECTED): return False, 'despesas incorretas'
    if type(report.get('total_cents')) is not int or report['total_cents'] != 98970: return False, 'total_cents incorreto'
    return True, 'relatório correto'

_GOOD = {'report': {'title': 'Visita cliente Recife - outubro', 'cost_center': 'Comercial', 'project': 'Aurora', 'expenses': [dict(zip(('date','category','description','amount_cents'), r)) for r in _EXPECTED], 'total_cents': 98970}}
WRONG = []
for key, value in [('title','Outro'), ('project','Boreal'), ('total_cents',98971), ('expenses',_GOOD['report']['expenses'][:4])]:
    case = deepcopy(_GOOD)
    case['report'][key] = value
    WRONG.append(case)
