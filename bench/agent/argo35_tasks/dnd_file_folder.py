"""Arquivos."""
ID = 'a35-dnd-file-folder'
CATEGORY = 'drag'
PAGE = 'argo35/dnd_file_folder.html'
PROMPT = "In 'Arquivos', move 'contrato-assinado.pdf' and 'nota-fiscal-0931.xml' into 'Financeiro 2026'. Leave every other file where it is."

def check(state, root):
    """Accept only the requested complete state."""
    try:
        ok = isinstance(state, dict) and (state == {'location': {'contrato-assinado.pdf': 'Financeiro 2026', 'nota-fiscal-0931.xml': 'Financeiro 2026', 'contrato-rascunho.pdf': None, 'relatorio.pdf': None, 'orcamento.xlsx': None, 'logo.png': None, 'ata.docx': None, 'foto.jpg': None, 'manual.pdf': None, 'dados.csv': None, 'proposta.pdf': None, 'recibo.pdf': None, 'agenda.txt': None, 'backup.zip': None}})
        return bool(ok), 'Estado correto' if ok else 'Estado incompleto ou alteração adicional'
    except (KeyError, TypeError, ValueError, AttributeError):
        return False, 'Estado inválido'

WRONG = [{'location': {'contrato-assinado.pdf': None, 'nota-fiscal-0931.xml': None, 'contrato-rascunho.pdf': None, 'relatorio.pdf': None, 'orcamento.xlsx': None, 'logo.png': None, 'ata.docx': None, 'foto.jpg': None, 'manual.pdf': None, 'dados.csv': None, 'proposta.pdf': None, 'recibo.pdf': None, 'agenda.txt': None, 'backup.zip': None}}, {'location': {'contrato-assinado.pdf': 'Financeiro 2026', 'nota-fiscal-0931.xml': None, 'contrato-rascunho.pdf': None, 'relatorio.pdf': None, 'orcamento.xlsx': None, 'logo.png': None, 'ata.docx': None, 'foto.jpg': None, 'manual.pdf': None, 'dados.csv': None, 'proposta.pdf': None, 'recibo.pdf': None, 'agenda.txt': None, 'backup.zip': None}}, {'location': {'contrato-assinado.pdf': 'Financeiro 2025', 'nota-fiscal-0931.xml': None, 'contrato-rascunho.pdf': None, 'relatorio.pdf': None, 'orcamento.xlsx': None, 'logo.png': None, 'ata.docx': None, 'foto.jpg': None, 'manual.pdf': None, 'dados.csv': None, 'proposta.pdf': None, 'recibo.pdf': None, 'agenda.txt': None, 'backup.zip': None}}]
