from tasks import norm, digits, expect
from tasks_hard import exact
from copy import deepcopy

ID = "long-onboarding"
PAGE = "long/onboarding.html"
PROMPT = "In the 'Admissão de colaborador' wizard, register this employee and confirm the admission. Personal data: name Larissa Gomes Pereira, CPF 123.456.789-09, birth date 14/03/1992, phone (31) 99876-5432, e-mail larissa.pereira@example.com, marital status Casado(a). Address: CEP 30140-071, street Rua da Bahia, number 1148, complement Apto 502, neighborhood Centro, city Belo Horizonte, state Minas Gerais. Bank: Itaú (341), branch 1234, account 56789-0, account type Corrente, Pix key type E-mail, Pix key larissa.pereira@example.com. Dependents: Marcos Pereira, Cônjuge, born 02/07/1990; and Sofia Gomes Pereira, Filho(a), born 21/11/2019."
_EXPECTED = {'name': 'Larissa Gomes Pereira', 'cpf': '123.456.789-09', 'birth': '14/03/1992', 'phone': '(31) 99876-5432', 'email': 'larissa.pereira@example.com', 'marital': 'Casado(a)', 'cep': '30140-071', 'street': 'Rua da Bahia', 'number': '1148', 'complement': 'Apto 502', 'neighborhood': 'Centro', 'city': 'Belo Horizonte', 'state': 'Minas Gerais', 'bank': '341 - Itaú', 'branch': '1234', 'account': '56789-0', 'accountType': 'Corrente', 'pixType': 'E-mail', 'pixKey': 'larissa.pereira@example.com', 'dependents': [{'name': 'Marcos Pereira', 'relation': 'Cônjuge', 'birth': '02/07/1990'}, {'name': 'Sofia Gomes Pereira', 'relation': 'Filho(a)', 'birth': '21/11/2019'}]}

def check(state, root):
    a = state.get('admission')
    if not isinstance(a, dict): return False, 'admission missing'
    checks=[]
    for k,v in _EXPECTED.items():
        if k=='dependents': continue
        transform = digits if k in ('cpf','phone','cep','account') else exact if k=='birth' else norm
        checks.append((k, '341' in str(a.get(k,'')) if k=='bank' else transform(a.get(k)), True if k=='bank' else transform(v)))
    ok, detail=expect(checks)
    if not ok: return ok, detail
    ds=a.get('dependents')
    if not isinstance(ds,list) or len(ds)!=2 or not all(isinstance(d,dict) for d in ds): return False,'dependents must contain exactly two people'
    key=lambda d:(norm(d.get('name')),norm(d.get('relation')),exact(d.get('birth')))
    return expect([('dependents',sorted(map(key,ds)),sorted(map(key,_EXPECTED['dependents'])))])

WRONG = [{}]
for _key, _value in [('cpf','123'),('bank','237 - Bradesco'),('dependents',[]),('accountType','Poupança')]:
    _a=deepcopy(_EXPECTED)
    _a[_key]=_value
    WRONG.append({'admission':_a})
