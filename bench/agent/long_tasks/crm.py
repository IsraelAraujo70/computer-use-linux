"""CRM state contract and independent checker."""
from copy import deepcopy
from tasks import norm, expect
ID = 'long-crm'
PAGE = 'long/crm.html'
PROMPT = "In the CRM, mark these leads as clients (status 'Cliente') with these contract values: Ana Ribeiro 12.500,00; Bruno Teixeira 8.900,00; Carla Menezes 23.000,00; Diego Antunes 4.750,00; Elisa Prado 15.300,00. Then delete the lead 'Rafael Souza (duplicado)'. Do not change any other lead."
INITIAL = {'lead-01': {'name': 'Ana Ribeiro', 'company': 'Ribeiro Arquitetura', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-02': {'name': 'Ana Ribeiro Lima', 'company': 'Lima Dental', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-03': {'name': 'Contato 03', 'company': 'Empresa 03', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-04': {'name': 'Bruno Teixeira', 'company': 'Teixeira Log', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-05': {'name': 'Contato 05', 'company': 'Empresa 05', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-06': {'name': 'Contato 06', 'company': 'Empresa 06', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-07': {'name': 'Contato 07', 'company': 'Empresa 07', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-08': {'name': 'Contato 08', 'company': 'Empresa 08', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-09': {'name': 'Carla Menezes', 'company': 'Menezes & Filhos', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-10': {'name': 'Contato 10', 'company': 'Empresa 10', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-11': {'name': 'Contato 11', 'company': 'Empresa 11', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-12': {'name': 'Contato 12', 'company': 'Empresa 12', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-13': {'name': 'Rafael Souza', 'company': 'Souza Consultoria', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-14': {'name': 'Contato 14', 'company': 'Empresa 14', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-15': {'name': 'Contato 15', 'company': 'Empresa 15', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-16': {'name': 'Contato 16', 'company': 'Empresa 16', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-17': {'name': 'Contato 17', 'company': 'Empresa 17', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-18': {'name': 'Diego Antunes', 'company': 'Antunes Tech', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-19': {'name': 'Contato 19', 'company': 'Empresa 19', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-20': {'name': 'Contato 20', 'company': 'Empresa 20', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-21': {'name': 'Contato 21', 'company': 'Empresa 21', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-22': {'name': 'Contato 22', 'company': 'Empresa 22', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-23': {'name': 'Contato 23', 'company': 'Empresa 23', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-24': {'name': 'Elisa Prado', 'company': 'Prado Eventos', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-25': {'name': 'Contato 25', 'company': 'Empresa 25', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-26': {'name': 'Contato 26', 'company': 'Empresa 26', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-27': {'name': 'Contato 27', 'company': 'Empresa 27', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-28': {'name': 'Rafael Souza (duplicado)', 'company': 'Souza Consultoria', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-29': {'name': 'Contato 29', 'company': 'Empresa 29', 'status': 'Novo', 'value_cents': 0, 'notes': ''}, 'lead-30': {'name': 'Contato 30', 'company': 'Empresa 30', 'status': 'Novo', 'value_cents': 0, 'notes': ''}}
TARGETS = dict(zip(['lead-01','lead-04','lead-09','lead-18','lead-24'], [1250000,890000,2300000,475000,1530000]))
EXPECTED = deepcopy(INITIAL)
for key, value in TARGETS.items():
    EXPECTED[key].update(status='Cliente', value_cents=value)
del EXPECTED['lead-28']

def check(state, root):
    leads = state.get('leads')
    if not isinstance(leads, dict):
        return False, 'missing leads'
    if set(leads) != set(EXPECTED):
        return False, 'missing, extra, or undeleted lead'
    for key, wanted in EXPECTED.items():
        row = leads[key]
        if not isinstance(row, dict):
            return False, key + ': invalid lead'
        for field, value in wanted.items():
            actual = row.get(field)
            if field == 'status':
                actual, value = norm(actual), norm(value)
            if actual != value:
                return False, key + ': wrong ' + field
    return True, 'all requested changes saved; other leads unchanged'

WRONG = [{'leads': deepcopy(INITIAL)}]
for key, field, value in [('lead-01','value_cents',12500), ('lead-18','status','Novo'), ('lead-13','notes','alterado'), ('lead-02','status','Cliente')]:
    case = deepcopy(EXPECTED)
    case[key][field] = value
    WRONG.append({'leads': case})
