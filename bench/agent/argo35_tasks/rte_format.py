from html.parser import HTMLParser
ID = 'a35-rte-format'
CATEGORY = 'rich-text'
PAGE = 'argo35/rte_format.html'
PROMPT = "In Aurora Comunicado da equipe, make 'prazo final' bold, turn Item 1, Item 2 and Item 3 into a bulleted list, link 'portal' to https://portal.example.com, then click 'Salvar'. Keep all text unchanged."
class Document(HTMLParser):
    def __init__(self):
        super().__init__(); self.stack=[]; self.text=[]; self.bold=[]; self.links=[]; self.items=[]; self.uls=0; self.bad=False
    def handle_starttag(self, tag, attrs):
        if tag not in ('p','div','br','b','strong','ul','li','a'): self.bad=True
        if tag=='ul': self.uls+=1
        attrs=dict(attrs)
        if tag=='a': self.links.append([attrs.get('href'), ''])
        if tag=='li':
            if 'ul' not in self.stack: self.bad=True
            self.items.append('')
        self.stack.append(tag)
        if tag in ('b','strong'): self.bold.append('')
    def handle_endtag(self, tag):
        if tag in self.stack:
            self.stack=self.stack[:len(self.stack)-1-self.stack[::-1].index(tag)]
    def handle_data(self, data):
        self.text.append(data)
        if any(t in self.stack for t in ('b','strong')) and self.bold: self.bold[-1]+=data
        if 'a' in self.stack and self.links: self.links[-1][1]+=data
        if 'li' in self.stack and self.items: self.items[-1]+=data

def check(state, root):
    if not isinstance(state,dict) or set(state)!= {'html'} or not isinstance(state['html'],str): return False,'Missing HTML'
    try:
        doc=Document(); doc.feed(state['html'])
        ok=not doc.bad and ''.join(doc.text)=='O prazo final está no portal.Item 1Item 2Item 3' and doc.bold==['prazo final'] and doc.uls==1 and doc.items==['Item 1','Item 2','Item 3'] and doc.links==[['https://portal.example.com','portal']]
        return ok,'Exact formatting and unchanged text required'
    except Exception:
        return False,'Invalid HTML'
WRONG=[{'html':'<p>O prazo final está no portal.</p><p>Item 1</p><p>Item 2</p><p>Item 3</p>'},{'html':'<b>prazo final</b>'},{'html':'<ul><li>Item 1</li></ul>'}]
