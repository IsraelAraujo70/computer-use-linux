"""Independent download_csv task contract."""
ID = "a35-download-csv"
CATEGORY = 'upload-download'
PAGE = "argo35/download_csv.html"
PROMPT = "In the Relatórios de vendas page, Download the CSV for 'Sul' in 'Outubro 2026' using 'Baixar CSV'."
import csv
FILENAME='relatorio-sul-2026-10.csv'
ROWS=[['pedido','regiao','periodo','valor'],['4821','Sul','2026-10','1250.00'],['4822','Sul','2026-10','890.00'],['4823','Sul','2026-10','430.00']]
def check(state, root):
    if not isinstance(state,dict) or state.get('downloaded')!=FILENAME or type(state.get('rows')) is not int or state['rows']!=3: return False,'wrong report'
    try:
        with (root/'downloads'/FILENAME).open(newline='') as f: actual=list(csv.reader(f))
        return (actual==ROWS,'CSV content required')
    except (OSError,UnicodeError,csv.Error): return False,'missing or invalid CSV'
WRONG=[{'downloaded':'relatorio-norte-2026-10.csv','rows':3},{'downloaded':FILENAME,'rows':0},{'downloaded':FILENAME,'rows':4}]
