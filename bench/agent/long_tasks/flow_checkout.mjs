export const TASK_ID = 'long-checkout';
export async function flow({ page, base, assert }) {
  await page.goto(base + 'checkout.html');
  for (const [name, qty, search] of [
    ['Caneca Aurora 300 ml', '2', 'Caneca'],
    ['Caderno pontilhado A5', '3', 'Caderno'],
    ['Garrafa térmica 750 ml', '1', 'Garrafa'],
    ['Lápis grafite 2B', '4', 'Lápis'],
  ]) {
    await page.getByLabel('Buscar produtos').fill(search);
    await page.getByLabel('Quantidade de ' + name, { exact: true }).fill(qty);
    await page.getByRole('button', { name: 'Adicionar ' + name + ' ao carrinho', exact: true }).click();
  }
  await page.getByLabel('Cupom', { exact: true }).fill('BEMVINDO10');
  await page.getByRole('button', { name: 'Aplicar cupom' }).click();
  await page.getByRole('button', { name: 'Ir para entrega' }).click();
  await page.getByRole('button', { name: 'Ir para pagamento' }).click();
  assert(await page.getByRole('alert').filter({ hasText: 'Preencha um CEP válido' }).isVisible(), 'missing CEP blocks checkout');
  assert(!(await page.getByRole('button', { name: 'Finalizar pedido' }).isVisible()), 'payment stays hidden');
  for (const [label, value] of [['CEP', '01310-100'], ['Rua', 'Avenida Paulista'], ['Número', '1578'], ['Complemento', 'Conjunto 12'], ['Cidade', 'São Paulo']]) {
    await page.getByLabel(label, { exact: true }).fill(value);
  }
  await page.getByLabel('Estado', { exact: true }).selectOption({ label: 'São Paulo' });
  await page.getByRole('radio', { name: 'Expressa R$ 24,90', exact: true }).click();
  await page.getByRole('button', { name: 'Ir para pagamento' }).click();
  await page.getByRole('radio', { name: 'Pix', exact: true }).click();
  await page.getByRole('button', { name: 'Finalizar pedido' }).click();
  await page.getByRole('status').filter({ hasText: 'Pedido realizado' }).waitFor();
}
