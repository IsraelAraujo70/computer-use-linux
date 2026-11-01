"""Independent dnd_upload_zone task contract."""
ID = "a35-dnd-upload-zone"
CATEGORY = 'drag'
PAGE = "argo35/dnd_upload_zone.html"
PROMPT = "Attach /tmp/agent-cu-bench/files/print-erro.png and /tmp/agent-cu-bench/files/log-servidor.txt, then click 'Enviar anexos'."
import hashlib
FILES = {'print-erro.png': b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x04\x00\x00\x00\xb5\x1c\x0c\x02\x00\x00\x00\x0bIDATx\xdac\xfc\xff\x1f\x00\x03\x03\x02\x00\xef\xa3E\x99\x00\x00\x00\x00IEND\xaeB`\x82', 'log-servidor.txt': b'Servidor: falha HTTP 503\n', 'log-antigo.txt': b'Arquivo antigo\n'}
NAMES = ['print-erro.png', 'log-servidor.txt']
def setup(root):
    folder = root / 'files'
    folder.mkdir(parents=True, exist_ok=True)
    for name, data in FILES.items(): (folder / name).write_bytes(data)
def check(state, root):
    if not isinstance(state, dict) or not isinstance(state.get('sent'), list): return False, 'missing sent files'
    try:
        expected = [dict(name=n, size=(root/'files'/n).stat().st_size, sha256=hashlib.sha256((root/'files'/n).read_bytes()).hexdigest()) for n in NAMES]
        return (sorted(state['sent'], key=lambda f:f['name']) == sorted(expected,key=lambda f:f['name']), 'exact file names, sizes and hashes required')
    except (OSError, TypeError, KeyError): return False, 'invalid files'
WRONG = [{'sent':[]}, {'sent':[{'name':NAMES[0]}]}, {'sent':[{'name':'log-antigo.txt','size':0,'sha256':''}]}]
