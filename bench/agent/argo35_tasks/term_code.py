"""Independent term_code task contract."""
ID = "a35-term-code"
CATEGORY = 'multi-window'
PAGE = "argo35/term_code.html"
PROMPT = "In the Verificação em duas etapas page, the terminal is open in a folder containing codigo-verificacao.txt. Read codigo-verificacao.txt in the terminal's current folder, enter it in 'Código de verificação' and click 'Verificar'."
import time
KIND = 'web+terminal'
def setup(root):
    folder=root/'term'
    folder.mkdir(parents=True,exist_ok=True)
    (folder/'codigo-verificacao.txt').write_text(str(time.time_ns() % 900000 + 100000)+'\n')
    (folder/'codigo-antigo.txt').write_text('000000\n')
def check(state,root):
    try:
        code=(root/'term'/'codigo-verificacao.txt').read_text().strip()
        return (isinstance(state,dict) and state.get('code') == code and len(code)==6 and code.isdigit(), 'current episode code required')
    except OSError: return False, 'missing verification file'
WRONG = [{'code':''},{'code':'000000'},{'code':'abcdef'}]
