export const TASK_ID = 'long-triage';
export async function flow({ page, base, assert }) {
  await page.goto(base + 'triage.html');
  await page.getByRole('button', {name:'Arquivar', exact:true}).click();
  assert(await page.getByRole('alert').textContent() === 'Selecione pelo menos uma mensagem.', 'selection validation');
  await page.getByLabel("Selecionar Banco Aurora - Fatura de outubro", {exact:true}).check();
  await page.getByLabel("Selecionar Banco Aurora - Extrato mensal", {exact:true}).check();
  await page.getByLabel("Selecionar Banco Aurora - Fatura do cartão", {exact:true}).check();
  await page.getByLabel("Selecionar Banco Aurora - Extrato de investimentos", {exact:true}).check();
  await page.getByRole('button', {name:'Rótulos', exact:true}).click();
  await page.getByRole('menuitemcheckbox', {name:'Financeiro', exact:true}).click();
  await page.getByLabel("Selecionar Ana Ribeiro - Pedido #201", {exact:true}).check();
  await page.getByLabel("Selecionar Carlos Silva - Pedido #202", {exact:true}).check();
  await page.getByLabel("Selecionar Oficina Azul - Pedido #203", {exact:true}).check();
  await page.getByLabel("Selecionar Livraria Central - Pedido #204", {exact:true}).check();
  await page.getByRole('button', {name:'Rótulos', exact:true}).click();
  await page.getByRole('menuitemcheckbox', {name:'Clientes', exact:true}).click();
  await page.getByLabel("Selecionar Loja Vértice - Promoção da semana", {exact:true}).check();
  await page.getByLabel("Selecionar Loja Vértice - Descontos especiais", {exact:true}).check();
  await page.getByLabel("Selecionar Loja Vértice - Oferta de primavera", {exact:true}).check();
  await page.getByLabel("Selecionar Loja Vértice - Cupom exclusivo", {exact:true}).check();
  await page.getByLabel("Selecionar Loja Vértice - Últimas ofertas", {exact:true}).check();
  await page.getByRole('button', {name:'Arquivar', exact:true}).click();
  await page.getByRole('group', {name:"Marina Costa - Almoço amanhã", exact:true}).getByRole('button', {name:'Marcar com estrela', exact:true}).click();
  await page.getByRole('group', {name:"Marina Costa - Fotos da viagem", exact:true}).getByRole('button', {name:'Marcar com estrela', exact:true}).click();
  await page.getByRole('group', {name:"Marina Costa - Reunião de família", exact:true}).getByRole('button', {name:'Marcar com estrela', exact:true}).click();
  await page.getByRole('status').filter({hasText:'Alterações salvas'}).waitFor();
}
