# argo35 task specs

The contract for each module is in `__init__.py`. This file fixes what each task is. Pages live in
`bench/agent/apps/argo35/<module>.html` (extra assets: `apps/argo35/<module>-*.html|js`), use
`../style.css` and `../common.js` (`postState(taskId, obj)`, `toast`, `h`), and post the task's
**full current state** on every meaningful change (so a timed-out episode is judged on what it did).
Pages never post on load.

## Rules for every page

- Brazilian Portuguese UI, realistic look (a real product, not a test page). The Chrome window is
  ~1900x1000 CSS px (a `web+terminal` task gets about half the width: lay out for 900 px).
- Deterministic: no `Math.random` / time-dependent content; generate data with a seeded PRNG.
- Normal DOM controls get proper labels/roles. What the spec calls *pointer-only* has no keyboard,
  button, or form alternative: it reacts to real pointer input only (pointerdown/move/up or HTML5 DnD
  as the spec says). Canvas content is pixels only: the `<canvas>` gets `role="img"` and a generic
  `aria-label` (e.g. "Mapa das lojas"), never the data.
- No hidden shortcuts that make the task trivial (no `?debug`, no listing of canvas data in the DOM).
- The checker is strict: success only for exactly the requested end state and no collateral change.
  Partial progress must fail. The checker reads the posted state (and files under `root` where the
  spec says so) and nothing else. For tasks that also read files, the checker must still require
  the posted state, so `{}` fails even when the file is present.
- `WRONG`: at least 3 realistic wrong/partial states (wrong item, extra change, half done, ...).
- The flow drives the visible UI. For canvas/drag it may read geometry via `page.evaluate`
  (e.g. getBoundingClientRect, or a documented `window.__geometry` read-only object the page exposes
  for the selftest) but must act with `page.mouse` / `page.keyboard` / locators.

## Tasks

### drag
1. **dnd_reorder** `a35-dnd-reorder` "Prioridades da sprint": sortable list of 8 tasks, reorder by
   dragging the row handle (⋮⋮) with *pointer events* (pointerdown on handle, a 6 px threshold,
   a placeholder while moving, drop on pointerup). Pointer-only. Prompt: move "Migrar banco de dados"
   to the top and "Atualizar dependências" to the bottom, keep the others' relative order. State
   `{order:[ids]}` after each drop. Check exact order.
2. **dnd_file_folder** `a35-dnd-file-folder` file manager "Arquivos" (icon grid, ~14 files, 4 folders),
   *HTML5 drag and drop* (draggable=true, dragover/drop on folders). Pointer-only (no move menu).
   Prompt: move "contrato-assinado.pdf" and "nota-fiscal-0931.xml" into folder "Financeiro 2026".
   Decoy: "contrato-rascunho.pdf", folder "Financeiro 2025". State `{location:{file: folder|null}}`.
3. **dnd_slider** `a35-dnd-slider` "Equalizador / Configurações de áudio": 5 custom div sliders
   (track 400 px, 0–100, integer, readout next to each). Pointer-only: drag the thumb or click the
   track; no keyboard, not `<input type=range>`. Prompt: set 'Volume' to 35 and 'Graves' to 80,
   leave the others. Check volume in [34,36], graves in [79,81], others exactly initial.
   Then click 'Aplicar' (state posted on Aplicar, with `applied:true`).
4. **dnd_resize_panel** `a35-dnd-resize-panel` IDE-like layout: left panel 'Explorador' and bottom
   panel 'Terminal', each resized by dragging a 6 px splitter (pointer events, cursor col/row-resize).
   Readouts "Explorador: N px" / "Terminal: N px" in a status bar. Prompt: make Explorador 600 px wide
   and Terminal 300 px tall. Check |w-600|<=10 and |h-300|<=10. State posted on every pointerup.
5. **dnd_kanban_pointer** `a35-dnd-kanban-pointer` board with 4 columns ('A fazer', 'Em andamento',
   'Em revisão', 'Concluído'), ~12 cards, *pointer-event* drag (not HTML5; 5 px threshold, card follows
   pointer, drop index by pointer y). Pointer-only. Prompt: move "Corrigir login SSO" from 'Em revisão'
   to the top of 'Concluído', and "Escrever testes E2E" from 'A fazer' to 'Em andamento' (any position).
   State `{columns:{col:[card ids in order]}}`. Check those two moves exactly, others unchanged in
   column and relative order.
6. **dnd_upload_zone** `a35-dnd-upload-zone` "Anexos do chamado #4821": a drop zone only (no
   `<input type=file>` anywhere, no button to browse): it takes files dropped from the OS
   (dragenter/dragover/drop with `dataTransfer.files`). `setup(root)` writes
   root/files/print-erro.png (a valid small PNG) and root/files/log-servidor.txt (known text) plus
   a decoy root/files/log-antigo.txt. Prompt (absolute paths under /tmp/agent-cu-bench/files/):
   attach print-erro.png and log-servidor.txt, then click 'Enviar anexos'. State on Enviar:
   `{sent:[{name,size,sha256}]}`. Check exactly those two files by name+sha256 (compute from the files
   setup wrote under `root`). The flow uses `page.dispatchEvent` of a drop with a DataTransfer built
   from the file bytes (Playwright cannot do OS drops) — that is the one allowed exception.

### canvas
7. **canvas_draw_rect** `a35-canvas-draw-rect` drawing app: DOM toolbar ('Selecionar', 'Retângulo',
   'Elipse', 'Texto' buttons), 1200x700 canvas. The canvas shows a dashed target outline labelled
   "Área do logo" drawn in pixels. Prompt: with the 'Retângulo' tool draw a rectangle that covers the
   "Área do logo" outline, then click 'Salvar'. State on Salvar `{shapes:[{type,x,y,w,h}]}` in canvas
   coordinates. Check exactly one shape, a rect, IoU with the target >= 0.8.
8. **canvas_map_pin** `a35-canvas-map-pin` store locator: canvas map with ~12 pins and their labels
   drawn as text in pixels ("Loja Centro", "Loja Centro-Oeste", "Loja Centro Histórico", ...).
   Clicking a pin (hit radius 14 px) shows the store in a DOM side panel with 'Definir como minha loja'.
   Prompt: set "Loja Centro" as my store. State on that button `{my_store: id}`. Check id.
9. **canvas_connect** `a35-canvas-connect` whiteboard (Figma/Miro-like) canvas: 5 rounded boxes with
   text drawn in pixels ("Pedido recebido", "Pagamento aprovado", "Enviar pedido", "Cancelar pedido",
   "Estornar"). DOM toolbar with 'Conector'. With the connector tool, a drag from inside one box to
   inside another creates a directed arrow. Prompt: connect "Pedido recebido" → "Pagamento aprovado"
   and "Pagamento aprovado" → "Enviar pedido". State `{edges:[[from,to]]}` after each change. Check the
   edge set equals exactly those 2.
10. **canvas_seats** `a35-canvas-seats` cinema seat map on canvas: rows A–J, seats 1–14, row letters
    and seat numbers drawn on the axes, some seats taken (grey). Click toggles a free seat. DOM summary
    "2 assentos selecionados" and button 'Confirmar assentos'. Prompt: choose seats F7 and F8 and
    confirm. State on confirm `{seats:[...]}`. Check exactly {F7,F8}.
11. **canvas_chart** `a35-canvas-chart` sales dashboard: bar chart on canvas, 12 months (labels
    "Jan".."Dez" drawn on the x axis, no value labels on bars, a y axis with ticks). Clicking a bar
    opens a DOM detail panel for that month with 'Abrir relatório'. Prompt: open the report of the
    month with the highest sales (the tallest bar). The tallest must be clearly tallest (>=8% over the
    next). State on 'Abrir relatório' `{report_month: "Set"}`. Check that month.
12. **canvas_signature** `a35-canvas-signature` contract signing: signature pad canvas (600x200),
    checkbox 'Li e concordo com os termos', button 'Assinar contrato' (disabled until the pad has
    ink). Prompt: sign in the 'Assinatura' box (any signature) and sign the contract. State on submit
    `{signed:true, agreed:bool, stroke_px:number, span_x:number}`. Check agreed and stroke length
    >= 250 px and horizontal span >= 200 px (a dot or a short tick fails).

### virtual-list
13. **vlist_10k** `a35-vlist-10k` "Registros de auditoria": 10,000 rows ("Registro #1".."#10000"),
    fixed 36 px rows, virtualized (only ~40 rows in the DOM), each with an 'Aprovar' button named
    "Aprovar registro #N". No search, no pagination, no jump-to. Prompt: approve 'Registro #4000' and
    nothing else. State `{approved:[n]}`. Check [4000].
14. **vlist_logs** `a35-vlist-logs` log viewer: 10,000 lines of variable height (1–4 wrapped lines),
    virtualized, the search box disabled ("Busca indisponível no plano gratuito"). Exactly one line
    contains "ERR-7731" (around line 6,200); near-miss decoys "ERR-7713", "ERR-773". Each line has a
    'Marcar como resolvido' button. Prompt: mark the log line with error code ERR-7731 as resolved.
    State `{resolved:[line numbers]}`. Check exactly that line.
15. **vlist_grid** `a35-vlist-grid` inventory spreadsheet: 5,000 rows x 30 columns, both axes
    virtualized (sticky header and first column 'SKU'). Double-click a cell (or Enter) edits it,
    Enter commits, Escape cancels. Prompt: set 'Estoque' of SKU-03120 to 42. 'Estoque' is column 23,
    off-screen to the right initially. State `{edits:{"SKU-03120:Estoque":"42"}}` (every committed
    edit). Check exactly that one edit with value 42.
16. **vlist_infinite** `a35-vlist-infinite` social feed: posts "#1".."#400" with author names, loads
    20 more 700 ms after the user scrolls near the bottom (spinner "Carregando mais publicações…").
    Each post has a 'Curtir' button. Prompt: like post #237 (by Marina Duarte). Decoy: other Marina
    Duarte posts. State `{liked:[n]}`. Check [237].
17. **vlist_listbox** `a35-vlist-listbox` "Cidade de entrega": custom dropdown (button + popup
    `role=listbox`, virtualized, 2,500 alphabetical generated city names, no typeahead, no filter).
    Prompt: choose "Vila Nova do Sul" and click 'Salvar'. Put it around option 1,800; decoys
    "Vila Nova do Sul de Minas", "Vila Nova". State on Salvar `{city}`. Check exact.

### context-menu
18. **ctx_rename** `a35-ctx-rename` file manager list: right-click a file opens a custom context
    menu ('Abrir', 'Renomear', 'Duplicar', 'Excluir'); 'Renomear' turns the name into an inline input
    (Enter confirms). No other rename path. Prompt: rename "rascunho-v3.docx" to "proposta-final.docx".
    State `{files:{id:name}}`. Check that rename only.
19. **ctx_submenu** `a35-ctx-submenu` mail list: right-click a message → menu with 'Mover para ▸'
    whose submenu opens on hover, with a nested 'Projetos ▸' submenu (hover) containing 'Projeto
    Aurora', 'Projeto Boreal'. Prompt: move the message from "Fornecedor XPTO" with subject
    "Cotação de servidores" to folder Projetos > Projeto Aurora. Decoy: another XPTO message.
    State `{folders:{msg_id:folder}}`. Check that move only.

### shortcut
20. **kbd_gmail** `a35-kbd-gmail` inbox where actions exist only as keyboard shortcuts (no buttons):
    j/k move the cursor row, x selects, e archives selected (or the cursor row), s stars. A visible
    hint "Pressione ? para ver os atalhos" opens a shortcut sheet. Prompt: archive all messages with
    label "Promoções" (4 of 15) and star the message from "Banco Central". State `{archived:[ids],
    starred:[ids]}`. Check exact sets.
21. **kbd_palette** `a35-kbd-palette` code editor app whose settings change only through the command
    palette (Ctrl+K or Ctrl+Shift+P opens a filterable `role=combobox` + listbox of ~40 commands;
    Enter runs). Status bar shows the current settings. Prompt: turn on word wrap ('Ativar quebra de
    linha automática'), change the theme to 'Solarizado claro', and set the indentation to 2 spaces.
    State `{wrap:bool, theme, indent}` after each command. Check exactly those, others default.

### multi-window
22. **popup_oauth** `a35-popup-oauth` "Integrações": 'Conectar Google Drive' opens a popup window
    (`window.open(url,'oauth','width=520,height=640')`, page `popup_oauth-auth.html`) with an account
    chooser ("israel@empresa.com", "israel.pessoal@gmail.com") and 'Permitir'. The popup sends the
    result with `window.opener.postMessage` and closes; the main page shows "Conectado como …". Then
    'Salvar integrações'. Prompt: connect Google Drive with the israel@empresa.com account and save.
    State on save `{drive:{connected,account}}`. Check account.
23. **tabs_compare** `a35-tabs-compare` "Comparar fornecedores": three supplier links open in new
    tabs (`target=_blank`, page `tabs_compare-supplier.html?id=…`) each with price and code. The main
    page has 'Código do fornecedor' and 'Adicionar ao pedido'. Prompt: add to the order the supplier
    with the lowest total price (price + frete shown on each supplier page). State on submit
    `{supplier_code}`. Check the cheapest total's code (cheapest by unit price must differ).
24. **term_code** `a35-term-code` KIND `web+terminal`. `setup(root)` writes
    root/term/codigo-verificacao.txt with a 6-digit code derived from the current time (different per
    episode) and a decoy root/term/codigo-antigo.txt. The page "Verificação em duas etapas" has field
    'Código de verificação' and 'Verificar'. Prompt: the terminal window is open in a folder with the
    file codigo-verificacao.txt; read the code from it and enter it on the page, then 'Verificar'.
    State `{code}`. Check equals the file content under `root` (so `{}` fails).

### dialog
25. **native_dialogs** `a35-native-dialogs` "Projetos": each row has 'Excluir'. Excluir → native
    `confirm("Excluir o projeto …?")` → if OK, native `prompt("Digite o nome do projeto para
    confirmar")` → if it matches, delete and native `alert("Projeto excluído")`. Prompt: delete project
    "Projeto Legado" (decoy "Projeto Legado 2"). State `{projects:[names]}` on each deletion. Check
    exactly that one gone.
26. **dialog_nested** `a35-dialog-nested` app settings: a cookie banner (bottom, blocks the page with
    an overlay until 'Aceitar' or 'Recusar'). 'Configurações' opens a modal; change 'Fuso horário'
    (select) to 'America/Manaus'; 'Salvar' opens a nested confirm modal "Isso vai alterar 12
    agendamentos" with 'Confirmar' / 'Voltar'; after Confirmar a toast with 'Desfazer' (5 s).
    Prompt: set the time zone to America/Manaus, do not accept cookies. State `{timezone,
    cookies:'aceito'|'recusado'|null}`. Check timezone and cookies != 'aceito' (undo reverts).

### iframe
27. **iframe_nested** `a35-iframe-nested` help center with a chat widget in a cross-origin iframe
    (`http://localhost:<port>/apps/argo35/iframe_nested-widget.html`, like apps/hard/bank.html) that
    itself embeds the form in a nested same-origin iframe (`iframe_nested-form.html`): 'Número do
    pedido', 'Motivo' (select), 'Descrição', 'Enviar'. Prompt: open a ticket for order 48213, reason
    'Produto com defeito', description "Tela trincada na entrega". The innermost frame posts state.
    Check all three fields.
28. **iframe_scroll_terms** `a35-iframe-scroll-terms` signup step: terms of service in a 600 px-tall
    iframe with its own long scroll; the checkbox 'Li e aceito os termos' (in the iframe, at the
    bottom) is disabled until the iframe content is scrolled to the end; then 'Continuar' on the main
    page. Prompt: accept the terms and continue. State `{accepted:true, scrolled_to_end:true}` on
    Continuar. Check both.

### upload-download
29. **download_csv** `a35-download-csv` reports: filters 'Período' (select, 'Outubro 2026'…) and
    'Região' (select), button 'Baixar CSV' builds a Blob and downloads via `<a download>` with name
    `relatorio-<regiao>-<aaaa-mm>.csv`. Prompt: download the CSV for region Sul, October 2026.
    State on download `{downloaded:filename, rows:n}`. Check: state names
    relatorio-sul-2026-10.csv and that file exists in root/downloads with the right header and row
    count. The flow saves the Playwright download to root/downloads/<suggestedFilename>.
30. **upload_hidden_input** `a35-upload-hidden-input` "Documentos": a styled 'Selecionar arquivos'
    label over a visually hidden `<input type=file multiple accept="image/*">`; a list of chosen
    files with remove (×); 'Enviar documentos'. `setup(root)` writes root/files/rg-frente.jpg,
    rg-verso.jpg (small valid JPEGs) and decoy rg-frente-antigo.jpg. Prompt (absolute paths): upload
    rg-frente.jpg and rg-verso.jpg and send. State on send `{sent:[{name,size,sha256}]}`. Check exact
    set by sha256 of the setup files.

### table / rich-text / date-picker / hover / keyboard-only
31. **table_sort_edit** `a35-table-sort-edit` HR table, 60 employees, 10 per page, sortable columns,
    'Departamento' filter, a row '⋯' menu with 'Editar' opening a modal (status select
    'Ativo'/'Promovido'/'Afastado'). Prompt: set status 'Promovido' for the employee with the highest
    salary in department 'Financeiro'. The highest Financeiro salary must not be on page 1 by default
    and must not be the highest overall. State `{status:{emp_id:status}}` on every save. Check only
    that employee changed.
32. **rte_format** `a35-rte-format` rich-text editor (contenteditable) with toolbar (Negrito,
    Itálico, Lista com marcadores, Link — link asks the URL in a small popover input) and Ctrl+B/I.
    Initial text: a paragraph containing "prazo final" and the word "portal", then three lines
    starting "Item 1", "Item 2", "Item 3". Prompt: make "prazo final" bold, turn the three Item lines
    into a bulleted list, link the word "portal" to https://portal.example.com, then 'Salvar'.
    State on save `{html}`. The checker parses html (html.parser): bold covers exactly "prazo final",
    one `<ul>` with exactly the 3 items, one `<a href=...>` with text "portal", text otherwise unchanged.
33. **datepicker_range** `a35-datepicker-range` hotel search: 'Check-in — Check-out' readonly input
    opening a two-month calendar popup (prev/next month buttons, day buttons with aria-label
    "14 de março de 2027"); first click = check-in, second = check-out. Opens on November 2026.
    Prompt: check-in 14/03/2027, check-out 02/04/2027, 2 adults, then 'Buscar'. State on Buscar
    `{checkin:"2027-03-14", checkout:"2027-04-02", adults:2}`. Check exact.
34. **hover_tooltip** `a35-hover-tooltip` pricing page, 4 plans; each plan's feature rows have an
    'ⓘ' icon; the explanation appears only while the pointer hovers it (inserted on mouseenter,
    removed on mouseleave; not on focus, no title attribute, nothing in the DOM before hover).
    Exactly one plan's tooltip on "Suporte" says "Suporte 24h por telefone" (others: "e-mail em até
    48h", "chat em horário comercial", ...). Prompt: subscribe to the plan whose support includes
    24-hour phone support ('Assinar' on that plan). It must not be the most expensive nor the
    highlighted plan. State `{plan}`. Check.
35. **kbd_only_otp** `a35-kbd-only-otp` two-step login: an "SMS" panel on the page shows "Seu código:
    482913". Six one-digit boxes that accept only real key presses: digits handled in keydown,
    `paste` prevented, and an `input` event that did not come from a keydown resets the box. Also a
    4-digit 'PIN do cartão' masked field with the same rule (PIN given in the prompt: 7350). Then
    'Confirmar'. State on Confirmar `{otp, pin}`. Check otp=="482913", pin=="7350".
