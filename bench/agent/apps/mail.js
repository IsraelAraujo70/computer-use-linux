// Gmail-like inbox shared by mail-reply.html and mail-label.html.
// window.MAIL = { task, prefix, special: {index: [sender, email, subject]} } selects the data set.
(function () {
  const cfg = window.MAIL;
  const rnd = seeded(cfg.seed);
  const senders = [
    ['Equipe Notion', 'team@example.com'], ['Paulo Mendes', 'paulo.mendes@example.com'],
    ['LinkedIn', 'messages@example.com'], ['Fernanda Lima', 'fernanda.lima@example.com'],
    ['GitHub', 'noreply@example.com'], ['Ricardo Alves', 'ricardo.alves@example.com'],
    ['Loja Vértice', 'pedidos@example.com'], ['Camila Rocha', 'camila.rocha@example.com'],
    ['Google Agenda', 'calendar@example.com'], ['Bruno Teixeira', 'bruno.t@example.com'],
    ['Condomínio Solar', 'sindico@example.com'], ['Juliana Prado', 'juliana.prado@example.com'],
  ];
  const subjects = [
    'Atualização do projeto', 'Reunião de quinta', 'Seu pedido foi enviado', 'Convite: revisão trimestral',
    'Fotos da viagem', 'Relatório semanal', 'Nova mensagem de recrutador', 'Lembrete de pagamento do condomínio',
    'Proposta comercial', 'Feedback sobre o protótipo', 'Ingressos confirmados', 'Pull request aprovado',
    'Documentos pendentes', 'Agenda da próxima semana', 'Orçamento da reforma', 'Newsletter de outubro',
  ];
  const snippets = [
    'Oi, tudo bem? Segue em anexo o material que conversamos ontem', 'Podemos remarcar para o fim da tarde?',
    'Confira os detalhes abaixo e responda se tiver dúvidas', 'Obrigado pelo retorno rápido, vou analisar',
    'Lembrando que o prazo termina na sexta-feira', 'Seguem os números consolidados do período',
  ];
  const months = ['out.', 'out.', 'out.', 'set.'];
  const messages = [];
  for (let i = 1; i <= 30; i++) {
    let [from, email] = senders[Math.floor(rnd() * senders.length)];
    let subject = subjects[Math.floor(rnd() * subjects.length)];
    const special = cfg.special[i];
    if (special) [from, email, subject] = special;
    const day = Math.max(1, 29 - Math.floor((i - 1) * 0.9));
    messages.push({
      id: cfg.prefix + i, from, email, subject,
      snippet: snippets[Math.floor(rnd() * snippets.length)],
      date: i <= 3 ? `${10 + (i * 7) % 12}:${String((i * 17) % 60).padStart(2, '0')}` : `${day} de ${months[i % 4 === 0 ? 3 : 0]}`,
      unread: rnd() < 0.35,
      labels: [],
    });
  }
  const state = { sent: [], archived: [], labels: {} };
  const selected = new Set();
  const panel = document.getElementById('panel');

  function publish() {
    const out = cfg.task === 'mail-reply'
      ? { sent: state.sent, archived: state.archived }
      : { labels: Object.fromEntries(messages.filter(m => m.labels.length).map(m => [m.id, m.labels])), archived: state.archived };
    return postState(cfg.task, out);
  }

  function renderInbox() {
    panel.replaceChildren();
    const visible = messages.filter(m => !state.archived.includes(m.id));
    const toolbar = h('div', { class: 'toolbar', role: 'toolbar', 'aria-label': 'Ações da caixa de entrada' });
    if (cfg.task === 'mail-label') {
      const all = h('input', { type: 'checkbox', 'aria-label': 'Selecionar todas' });
      all.checked = visible.length > 0 && visible.every(m => selected.has(m.id));
      all.addEventListener('change', () => {
        visible.forEach(m => (all.checked ? selected.add(m.id) : selected.delete(m.id)));
        renderInbox();
      });
      toolbar.append(all,
        h('button', { class: 'btn', onclick: archiveSelected }, 'Arquivar'),
        h('button', { class: 'btn', id: 'labels-btn', 'aria-haspopup': 'menu', 'aria-expanded': 'false', onclick: openLabelMenu }, 'Marcadores'));
    } else {
      toolbar.append(h('button', { class: 'icon-btn', 'aria-label': 'Atualizar' }, '⟳'),
        h('button', { class: 'icon-btn', 'aria-label': 'Mais' }, '⋮'));
    }
    toolbar.append(h('span', { class: 'spacer' }), h('span', {}, `1–${visible.length} de ${visible.length}`));
    const list = h('div', { role: 'list', 'aria-label': 'Mensagens' });
    for (const m of visible) {
      const row = h('div', { class: 'mailrow' + (m.unread ? ' unread' : ''), role: 'listitem', 'data-id': m.id });
      if (cfg.task === 'mail-label') {
        const box = h('input', { type: 'checkbox', 'aria-label': 'Selecionar: ' + m.subject });
        box.checked = selected.has(m.id);
        box.addEventListener('click', e => e.stopPropagation());
        box.addEventListener('change', () => { box.checked ? selected.add(m.id) : selected.delete(m.id); });
        row.append(box);
      }
      row.append(
        h('button', { class: 'icon-btn', 'aria-label': 'Com estrela', onclick: e => e.stopPropagation() }, '☆'),
        h('span', { class: 'from' }, m.from),
        h('span', { class: 'text' }, m.labels.map(l => h('span', { class: 'chip' }, l)), m.subject,
          h('span', { class: 'snippet' }, ' - ' + m.snippet)),
        h('span', { class: 'date' }, m.date));
      row.addEventListener('click', () => openMessage(m));
      list.append(row);
    }
    panel.append(toolbar, list);
  }

  function archiveSelected() {
    if (!selected.size) return;
    state.archived.push(...selected);
    selected.clear();
    publish();
    toast('Conversas arquivadas');
    renderInbox();
  }

  function openLabelMenu(event) {
    const btn = event.currentTarget;
    closeMenu();
    const menu = h('div', { class: 'menu', role: 'menu', id: 'labels-menu', 'aria-label': 'Marcar como' });
    const checks = {};
    for (const name of ['Financeiro', 'Pessoal', 'Trabalho', 'Viagem']) {
      const item = h('div', { role: 'menuitemcheckbox', 'aria-checked': 'false', tabindex: '-1' }, h('span', { 'aria-hidden': 'true' }, '☐'), name);
      item.addEventListener('click', () => {
        checks[name] = !checks[name];
        item.setAttribute('aria-checked', String(checks[name]));
        item.firstChild.textContent = checks[name] ? '☑' : '☐';
      });
      menu.append(item);
    }
    menu.append(h('div', { style: 'border-top:1px solid #eee;margin:6px 0' }),
      h('button', { class: 'btn primary', style: 'margin:4px 16px', onclick: () => {
        const names = Object.keys(checks).filter(k => checks[k]);
        for (const m of messages) if (selected.has(m.id)) for (const n of names) if (!m.labels.includes(n)) m.labels.push(n);
        closeMenu();
        if (names.length && selected.size) { publish(); toast('Marcadores aplicados'); }
        renderInbox();
      } }, 'Aplicar'));
    const rect = btn.getBoundingClientRect();
    const prect = panel.getBoundingClientRect();
    menu.style.left = rect.left - prect.left + 'px';
    menu.style.top = rect.bottom - prect.top + panel.scrollTop + 4 + 'px';
    btn.setAttribute('aria-expanded', 'true');
    panel.append(menu);
    setTimeout(() => document.addEventListener('click', outside), 0);
  }
  function outside(e) { const menu = document.getElementById('labels-menu'); if (menu && !menu.contains(e.target)) closeMenu(); }
  function closeMenu() {
    document.getElementById('labels-menu')?.remove();
    document.getElementById('labels-btn')?.setAttribute('aria-expanded', 'false');
    document.removeEventListener('click', outside);
  }

  function openMessage(m) {
    m.unread = false;
    panel.replaceChildren();
    const toolbar = h('div', { class: 'toolbar', role: 'toolbar' },
      h('button', { class: 'icon-btn', 'aria-label': 'Voltar para a caixa de entrada', onclick: renderInbox }, '←'),
      h('button', { class: 'btn', onclick: () => { state.archived.push(m.id); publish(); toast('Conversa arquivada'); renderInbox(); } }, 'Arquivar'));
    const body = h('div', { style: 'padding:16px 72px' },
      h('h1', { style: 'font-size:22px;font-weight:400' }, m.subject),
      h('div', {}, h('b', {}, m.from), ' ', h('span', { class: 'snippet' }, `<${m.email}>`), h('span', { style: 'float:right', class: 'snippet' }, m.date)),
      h('div', { style: 'margin:24px 0;white-space:pre-line' },
        `Olá, Israel,\n\n${m.snippet}. ${m.subject === 'Contrato de locação - assinatura' ? 'O contrato de locação do apartamento está pronto para assinatura; confirme o recebimento, por favor.' : 'Fico no aguardo do seu retorno quando puder.'}\n\nAbraços,\n${m.from}`),
      h('div', { style: 'display:flex;gap:8px' },
        h('button', { class: 'btn', onclick: () => openReply(m, body) }, '↩ Responder'),
        h('button', { class: 'btn' }, '↪ Encaminhar')));
    panel.append(toolbar, body);
  }

  function openReply(m, container) {
    if (container.querySelector('[role=textbox]')) return;
    const editor = h('div', { contenteditable: 'true', role: 'textbox', 'aria-multiline': 'true', 'aria-label': 'Corpo da mensagem',
      style: 'min-height:160px;border:1px solid #c4c7c5;border-radius:8px;padding:12px;margin:16px 0;outline:none' });
    const box = h('div', { role: 'region', 'aria-label': 'Resposta', style: 'margin-top:24px;border:1px solid #e0e0e0;border-radius:12px;padding:16px' },
      h('div', { class: 'snippet' }, 'Para: ' + m.email), editor,
      h('div', { style: 'display:flex;gap:8px' },
        h('button', { class: 'btn primary', onclick: async () => {
          state.sent.push({ in_reply_to: m.id, to: m.email, body: editor.innerText.trim() });
          await publish();
          toast('Mensagem enviada');
          renderInbox();
        } }, 'Enviar'),
        h('button', { class: 'icon-btn', 'aria-label': 'Descartar', onclick: () => box.remove() }, '🗑')));
    container.append(box);
    editor.focus();
  }

  renderInbox();
})();
