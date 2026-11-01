"""Loja Aurora checkout benchmark."""
from copy import deepcopy
from tasks import norm, digits, expect

ID = 'long-checkout'
PAGE = 'long/checkout.html'
PROMPT = "In Loja Aurora, buy: 2 x 'Caneca Aurora 300 ml', 3 x 'Caderno pontilhado A5', 1 x 'Garrafa térmica 750 ml' and 4 x 'Lápis grafite 2B'. Apply the coupon BEMVINDO10. Ship to CEP 01310-100, Avenida Paulista, number 1578, complement Conjunto 12, São Paulo, state São Paulo, with Expressa delivery, pay with Pix and place the order."
PRICES = {'Caneca Aurora 300 ml': 2990, 'Caderno pontilhado A5': 2490, 'Garrafa térmica 750 ml': 8990, 'Lápis grafite 2B': 350}
QUANTITIES = dict(zip(PRICES, [2, 3, 1, 4]))
TOTAL = sum(PRICES[n] * q for n, q in QUANTITIES.items()) * 9 // 10 + 2490


def check(state, root):
    order = state.get('order')
    if not isinstance(order, dict):
        return False, 'order missing'
    items = order.get('items')
    if not isinstance(items, list) or len(items) != 4:
        return False, 'expected exactly four items'
    seen = set()
    for item in items:
        if not isinstance(item, dict):
            return False, 'invalid item'
        name = item.get('name')
        if not isinstance(name, str) or name not in PRICES or name in seen:
            return False, 'wrong or duplicate product'
        seen.add(name)
        if type(item.get('qty')) is not int or item['qty'] != QUANTITIES[name]:
            return False, 'wrong quantity: ' + name
        if item.get('unit_cents') != PRICES[name]:
            return False, 'wrong price: ' + name
    address = order.get('address')
    if not isinstance(address, dict):
        return False, 'address missing'
    return expect([
        ('coupon', norm(order.get('coupon')), 'bemvindo10'),
        ('CEP', digits(address.get('cep')), '01310100'),
        ('street', norm(address.get('street')), norm('Avenida Paulista')),
        ('number', digits(address.get('number')), '1578'),
        ('complement', norm(address.get('complement')), 'conjunto 12'),
        ('city', norm(address.get('city')), norm('São Paulo')),
        ('state', norm(address.get('state')), norm('São Paulo')),
        ('delivery', norm(order.get('delivery')), 'expressa'),
        ('payment', norm(order.get('payment')), 'pix'),
        ('total_cents', order.get('total_cents'), TOTAL),
    ])


_correct = {'order': {'items': [{'name': n, 'qty': q, 'unit_cents': PRICES[n]} for n, q in QUANTITIES.items()], 'coupon': 'BEMVINDO10', 'address': {'cep': '01310-100', 'street': 'Avenida Paulista', 'number': '1578', 'complement': 'Conjunto 12', 'city': 'São Paulo', 'state': 'São Paulo'}, 'delivery': 'Expressa', 'payment': 'Pix', 'total_cents': TOTAL}}
WRONG = [{}]
for key, value in [('coupon', ''), ('delivery', 'Econômica'), ('payment', 'Boleto'), ('total_cents', TOTAL + 1)]:
    wrong = deepcopy(_correct)
    wrong['order'][key] = value
    WRONG.append(wrong)
wrong = deepcopy(_correct)
wrong['order']['items'][0]['qty'] = 1
WRONG.append(wrong)
wrong = deepcopy(_correct)
wrong['order']['items'][0]['name'] = 'Caneca Aurora 450 ml'
WRONG.append(wrong)
