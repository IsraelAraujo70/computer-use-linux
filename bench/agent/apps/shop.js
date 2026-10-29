// Orders data shared by orders.html, order.html and support.html.
const ORDERS = [];
(function () {
  const rnd = seeded(4821);
  const items = ['Cafeteira elétrica', 'Mochila de notebook', 'Teclado mecânico', 'Luminária de mesa', 'Garrafa térmica', 'Mouse sem fio', 'Fone de ouvido', 'Cadeira ergonômica'];
  const statuses = ['Entregue', 'Em trânsito', 'Entregue', 'Aguardando envio'];
  const letters = 'ABCDEFGHJKLMNPRSTUVWXZ';
  for (let n = 4810; n <= 4824; n++) {
    const tracking = n === 4821 ? 'BR482193XQ'
      : 'BR' + String(100000 + Math.floor(rnd() * 899999)) + letters[Math.floor(rnd() * letters.length)] + letters[Math.floor(rnd() * letters.length)];
    ORDERS.push({ n, date: `${String(1 + (n * 7) % 28).padStart(2, '0')}/10/2026`, item: items[Math.floor(rnd() * items.length)],
      total: (80 + Math.floor(rnd() * 900)) + ',90', status: n === 4821 ? 'Em trânsito' : statuses[Math.floor(rnd() * statuses.length)], tracking });
  }
})();
function siteBar() {
  return h('nav', { class: 'sitebar', 'aria-label': 'Loja' }, h('b', {}, 'Loja Vértice'),
    h('a', { href: 'orders.html' }, 'Meus pedidos'), h('a', { href: 'support.html' }, 'Abrir chamado'));
}
