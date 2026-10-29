// Drives each web task's happy path headlessly so the checkers are proven to
// accept a correct run (and reject the untouched page). Uses an existing
// playwright-core checkout: PLAYWRIGHT_CORE=/path/to/playwright-core node selftest.mjs <port>
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE);
const port = process.argv[2];
const base = `http://127.0.0.1:${port}/apps/`;
const exe = process.env.CHROME;
const browser = await chromium.launch({ executablePath: exe, headless: true });
const page = await browser.newPage({ viewport: { width: 1900, height: 950 } });
const errors = [];
page.on('pageerror', e => errors.push(e.message));
const reset = () => fetch(`http://127.0.0.1:${port}/api/reset`, { method: 'POST', body: '{}' });
const flows = {
  'mail-reply': async () => {
    await page.goto(base + 'mail-reply.html');
    await page.getByText('Contrato de locação - assinatura').click();
    await page.getByRole('button', { name: 'Responder' }).click();
    await page.getByRole('textbox', { name: 'Corpo da mensagem' }).fill('Recebido, obrigado.');
    await page.getByRole('button', { name: 'Enviar' }).click();
  },
  'mail-label': async () => {
    await page.goto(base + 'mail-label.html');
    await page.getByRole('checkbox', { name: 'Selecionar: Sua fatura de outubro chegou' }).check();
    await page.getByRole('checkbox', { name: 'Selecionar: Comprovante de transferência' }).check();
    await page.getByRole('button', { name: 'Marcadores' }).click();
    await page.getByRole('menuitemcheckbox', { name: 'Financeiro' }).click();
    await page.getByRole('button', { name: 'Aplicar' }).click();
  },
  'calendar-event': async () => {
    await page.goto(base + 'calendar.html');
    await page.getByRole('button', { name: 'Criar' }).click();
    await page.getByLabel('Título').fill('Dentista');
    await page.locator('#ev-date').click();
    await page.getByRole('dialog', { name: 'Escolher data' }).getByRole('button', { name: 'Próximo mês' }).click();
    await page.getByRole('button', { name: '18 de novembro de 2026' }).click();
    await page.getByRole('combobox', { name: 'Hora de início' }).click();
    await page.getByRole('option', { name: '14:30' }).first().click();
    await page.getByRole('button', { name: 'Salvar' }).click();
  },
  'form-signup': async () => {
    await page.goto(base + 'signup.html');
    await page.getByLabel('Nome completo').fill('Joana Ribeiro');
    await page.getByLabel('E-mail').fill('joana.ribeiro@example.com');
    await page.getByLabel('Telefone').fill('(31) 98765-4321');
    await page.getByRole('combobox', { name: 'Estado' }).click();
    await page.getByRole('option', { name: 'Minas Gerais' }).click();
    await page.getByLabel('Pro', { exact: true }).check();
    await page.getByLabel('Aceito os termos de uso').check();
    await page.getByRole('button', { name: 'Criar conta' }).click();
  },
  'settings-dialog': async () => {
    await page.goto(base + 'settings.html');
    await page.getByRole('button', { name: 'Mais opções' }).click();
    await page.getByRole('menuitem', { name: 'Configurações' }).click();
    await page.getByRole('tab', { name: 'Notificações' }).click();
    await page.getByLabel('Frequência').selectOption('weekly');
    await page.getByLabel('Notificações no navegador').check();
    await page.getByRole('button', { name: 'Salvar' }).click();
  },
  'contacts-infinite': async () => {
    await page.goto(base + 'contacts.html');
    const star = page.getByRole('button', { name: 'Marcar Valéria Nunes com estrela' });
    for (let i = 0; i < 40 && !(await star.count()); i++) { await page.locator('#sentinel').scrollIntoViewIfNeeded(); await page.waitForTimeout(450); }
    console.log('contacts target', await page.evaluate(() => window.__targetId));
    await star.click();
  },
  'iframe-billing': async () => {
    await page.goto(base + 'checkout.html');
    const f = page.frameLocator('iframe[title="Endereço de cobrança"]');
    await f.getByLabel('CEP').fill('30130-010');
    await f.getByLabel('Rua').fill('Avenida Afonso Pena');
    await f.getByLabel('Número').fill('1500');
    await f.getByLabel('Cidade').fill('Belo Horizonte');
    await f.getByLabel('Estado').selectOption('MG');
    await f.getByRole('button', { name: 'Confirmar endereço' }).click();
  },
  'order-support': async () => {
    await page.goto(base + 'orders.html');
    await page.getByRole('link', { name: '#4821' }).click();
    const code = (await page.getByText('Código de rastreio:').textContent()).split(': ')[1];
    await page.getByRole('link', { name: 'Abrir chamado' }).click();
    await page.getByLabel('Número do pedido').fill('4821');
    await page.getByLabel('Código de rastreio').fill(code);
    await page.getByLabel('Entrega').check();
    await page.getByLabel('Mensagem').fill('Pedido não chegou');
    await page.getByRole('button', { name: 'Enviar chamado' }).click();
  },
  'invoices-bulk': async () => {
    await page.goto(base + 'invoices.html');
    const rows = await page.locator('tbody tr').all();
    const acme = [];
    for (const r of rows) {
      const t = await r.innerText();
      if (t.includes('Acme Ltda') && t.includes('Vencida')) { acme.push(t.split('\t')[0].trim()); await r.getByRole('checkbox').check(); }
    }
    console.log('acme overdue', acme.join(','));
    await page.getByRole('button', { name: 'Arquivar selecionadas' }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Arquivar' }).click();
  },
};
for (const [task, flow] of Object.entries(flows)) {
  if (process.argv[3] && process.argv[3] !== task) continue;
  await reset();
  try { await flow(); await page.waitForTimeout(400); console.log('FLOW', task, 'done'); }
  catch (e) { console.log('FLOW', task, 'ERROR', e.message.split('\n')[0]); }
  const r = await fetch(`http://127.0.0.1:${port}/api/state/${task}`);
  console.log('STATE', task, JSON.stringify(await r.json()));
}
console.log('page errors:', errors.length ? errors : 'none');
await browser.close();
