// Shared helpers for the benchmark apps. postState sends the task's full state
// to server.py, which the checker reads; pages never post on load. Posts of one
// task go out one after another, so a slower earlier post can never land after
// (and overwrite) a later one.
const postChains = {};
function postState(task, obj) {
  const body = JSON.stringify(obj);
  const send = async () => {
    const response = await fetch('/api/state/' + task, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
    });
    if (!response.ok) throw new Error('Falha ao salvar');
  };
  const next = (postChains[task] || Promise.resolve()).catch(() => {}).then(send);
  postChains[task] = next;
  return next;
}

function toast(text) {
  let el = document.getElementById('toast');
  if (!el) {
    el = document.createElement('div');
    el.id = 'toast';
    el.setAttribute('role', 'status');
    document.body.appendChild(el);
  }
  el.textContent = text;
  el.className = 'show';
  clearTimeout(el._timer);
  el._timer = setTimeout(() => (el.className = ''), 4000);
}

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === 'class') el.className = value;
    else if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else if (value === true) el.setAttribute(key, '');
    else if (value !== false && value != null) el.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child == null) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

// Small deterministic PRNG so generated data is identical on every load.
function seeded(seed) {
  return () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
}
