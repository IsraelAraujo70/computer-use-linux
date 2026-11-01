"""Task contract and exact state checker."""
ID = 'a35-ctx-rename'
CATEGORY = 'context-menu'
PAGE = 'argo35/ctx_rename.html'
PROMPT = "In 'Arquivos', rename 'rascunho-v3.docx' to 'proposta-final.docx'. Leave all other files unchanged."
EXPECTED = {'files': {'f1': 'proposta-final.docx', 'f2': 'contrato.pdf', 'f3': 'rascunho-v2.docx', 'f4': 'orcamento.xlsx', 'f5': 'ata-reuniao.docx', 'f6': 'planejamento.pdf', 'f7': 'clientes.csv', 'f8': 'cronograma.xlsx', 'f9': 'marca.svg', 'f10': 'relatorio-outubro.pdf', 'f11': 'guia-equipe.pdf', 'f12': 'apresentacao.pptx', 'f13': 'custos.xlsx', 'f14': 'foto-equipe.png', 'f15': 'termos.pdf', 'f16': 'escopo.docx'}}

def check(state, root):
    if not isinstance(state, dict):
        return False, 'invalid state'
    valid = state == EXPECTED
    return valid, 'exact requested state' if valid else 'missing changes or collateral changes'

WRONG = [{'files': {'f1': 'rascunho-v3.docx', 'f2': 'contrato.pdf', 'f3': 'rascunho-v2.docx', 'f4': 'orcamento.xlsx'}}, {'files': {'f1': 'proposta-final.docx', 'f2': 'contrato.pdf', 'f3': 'proposta-final.docx', 'f4': 'orcamento.xlsx'}}, {'files': {'f1': 'proposta-final.docx'}}]
