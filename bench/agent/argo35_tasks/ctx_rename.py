"""Task contract and exact state checker."""
ID = 'a35-ctx-rename'
CATEGORY = 'context-menu'
PAGE = 'argo35/ctx_rename.html'
PROMPT = 'Rename "rascunho-v3.docx" to "proposta-final.docx" using its context menu. Leave all other files unchanged.'
EXPECTED = {'files': {'f1': 'proposta-final.docx', 'f2': 'contrato.pdf', 'f3': 'rascunho-v2.docx', 'f4': 'orcamento.xlsx'}}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'files': {'f1': 'rascunho-v3.docx', 'f2': 'contrato.pdf', 'f3': 'rascunho-v2.docx', 'f4': 'orcamento.xlsx'}}, {'files': {'f1': 'proposta-final.docx', 'f2': 'contrato.pdf', 'f3': 'proposta-final.docx', 'f4': 'orcamento.xlsx'}}, {'files': {'f1': 'proposta-final.docx'}}]
