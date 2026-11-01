ID = "a35-table-sort-edit"
CATEGORY = 'table'
PAGE = "argo35/table_sort_edit.html"
PROMPT = "In Aurora Pessoas e equipes, set 'Promovido' for the employee with the highest salary in 'Financeiro'. Keep every other field unchanged, then click 'Salvar'."
NAMES = ['Ana','Bruno','Carla','Diego','Elisa','Fábio','Gabriela','Hugo','Isabela','João']
SURNAMES = ['Almeida','Barros','Costa','Dias','Esteves','Freitas']
TARGET = {'employees': [dict(id=f'emp-{n+1}', name='Marina Duarte' if n == 46 else NAMES[n%10]+' '+SURNAMES[n//10], dept='Financeiro' if n%3 == 1 else 'Engenharia' if n%3 == 0 else 'Comercial', salary=18000 if n == 46 else 25000 if n == 59 else 3000+n*130, role='Analista', hired='2022-05-16', status='Promovido' if n == 46 else 'Ativo') for n in range(60)]}
def check(state, root):
    return isinstance(state, dict) and state == TARGET, 'All employee fields must match the requested change'
WRONG = [{}, {'employees': []}, {'employees': TARGET['employees'][:-1]}]
