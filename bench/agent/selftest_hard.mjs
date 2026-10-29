// Drives each hard task's happy path headlessly so the checkers are proven to accept a
// correct run, and asserts that every trap (flaky clear, late modal, slow page, failing
// first save, delayed suggestions, ...) really fires. Same setup as selftest.mjs:
// PLAYWRIGHT_CORE=/path/to/playwright-core CHROME=/path/to/chrome node selftest_hard.mjs <port> [task]
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE);
const here = dirname(fileURLToPath(import.meta.url));
const port = process.argv[2];
const base = `http://127.0.0.1:${port}/apps/hard/`;
const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
const context = await browser.newContext({ viewport: { width: 1900, height: 950 } });
const page = await context.newPage();
const errors = [];
const watch = p => p.on('pageerror', e => errors.push(`${p.url()}: ${e.message}`));
watch(page);
context.on('page', watch);
const reset = () => fetch(`http://127.0.0.1:${port}/api/reset`, { method: 'POST', body: '{}' });
const state = async task => (await fetch(`http://127.0.0.1:${port}/api/state/${task}`)).json();
const assert = (cond, msg) => { if (!cond) throw new Error('assertion failed: ' + msg); };
const since = t0 => Date.now() - t0;

const flows = {
  'accents-form': async () => {
    await page.goto(base + 'accents.html');
    await page.getByLabel('Nome completo').fill('João Conceição Araújo');
    await page.getByLabel('Cidade').fill('São Paulo');
    await page.getByLabel('Observações').fill('Preço: 1.234,56 € — ótimo! Über façade 😀 ação ênfase');
    await page.getByRole('button', { name: 'Salvar' }).click();
    await page.getByText('Cliente salvo').waitFor();
  },
  'rich-compose': async () => {
    await page.goto(base + 'compose.html');
    await page.getByRole('button', { name: 'Enviar', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'Adicione pelo menos um destinatário' }).waitFor();
    const to = page.getByRole('combobox', { name: 'Para' });
    await to.click();
    await to.pressSequentially('ana.souza@example');
    await page.keyboard.press('Enter');
    assert(await to.inputValue() === 'ana.souza@example', 'invalid address must stay as text');
    await to.pressSequentially('.com');
    await page.keyboard.press('Enter');
    await page.getByRole('button', { name: 'Remover ana.souza@example.com' }).waitFor();
    assert(await to.inputValue() === '', 'chip must take the typed text');
    await page.getByRole('textbox', { name: 'Assunto' }).fill('Relatório de outubro');
    await page.getByRole('textbox', { name: 'Corpo da mensagem' }).click();
    await page.keyboard.type('Olá, Ana!');
    await page.keyboard.press('Enter');
    await page.keyboard.type('Segue o relatório de outubro.');
    await page.getByRole('button', { name: 'Enviar', exact: true }).click();
    await page.getByText('Mensagem enviada').waitFor();
  },
  'flaky-field': async () => {
    await page.goto(base + 'payment.html');
    await page.getByLabel('Nome no cartão').fill('MARIA S OLIVEIRA');
    const card = page.getByLabel('Número do cartão');
    await card.click();
    await card.pressSequentially('4111111111111111');
    assert(await card.inputValue() === '4111 1111 1111 1111', 'mask groups digits by 4');
    await page.waitForTimeout(1400);
    assert(await card.inputValue() === '', 'card widget init must silently clear the field');
    await card.pressSequentially('4111111111111111');
    await page.getByLabel('Validade').pressSequentially('1228');
    assert(await page.getByLabel('Validade').inputValue() === '12/28', 'expiry mask');
    await page.getByLabel('CVV').fill('123');
    await page.waitForTimeout(1400);
    assert(await card.inputValue() === '4111 1111 1111 1111', 'second entry must stick');
    await page.getByRole('button', { name: 'Pagar' }).click();
    await page.getByText('Pagamento aprovado').waitFor();
  },
  'virtual-list': async () => {
    await page.goto(base + 'orders-virtual.html');
    const button = page.getByRole('button', { name: 'Arquivar pedido #1377', exact: true });
    const rows = await page.getByRole('listitem').count();
    assert(rows < 60 && !(await button.count()), `list must be virtualized (${rows} rows in DOM)`);
    for (let i = 0; i < 60 && !(await button.count()); i++) {
      await page.locator('#panel').evaluate(p => { p.scrollTop += 600; });
      await page.waitForTimeout(50);
    }
    await button.click();
    await page.getByText('Pedido #1377 arquivado').waitFor();
    assert(!(await button.count()), 'archived row disappears');
  },
  'modal-focus': async () => {
    await page.goto(base + 'contact.html');
    const t0 = Date.now();
    await page.getByLabel('Nome', { exact: true }).fill('Carlos Mendes');
    const dialog = page.getByRole('dialog', { name: 'Receba nossas novidades' });
    await dialog.waitFor({ timeout: 4000 });
    assert(since(t0) >= 1900, `modal opened too early (${since(t0)} ms)`);
    assert(await page.evaluate(() => document.activeElement.id === 'nl-email' && document.getElementById('site').inert),
      'modal must steal focus and make the page inert');
    await dialog.getByRole('button', { name: 'Agora não' }).click();
    await page.getByLabel('E-mail', { exact: true }).fill('carlos@example.com');
    await page.getByLabel('Mensagem').fill('Quero cancelar minha assinatura.');
    await page.getByRole('button', { name: 'Enviar mensagem' }).click();
    await page.getByText('Mensagem enviada').waitFor();
  },
  'slow-page': async () => {
    const t0 = Date.now();
    await page.goto(base + 'report.html');
    const generate = page.getByRole('button', { name: 'Gerar relatório' });
    assert(!(await generate.isVisible()), 'button must not be there while loading');
    await generate.waitFor({ timeout: 8000 });
    assert(since(t0) >= 4800, `page ready too early (${since(t0)} ms)`);
    const t1 = Date.now();
    await generate.click();
    await page.getByText('Gerando relatório…').waitFor();
    const exportMarch = page.getByRole('button', { name: 'Exportar Março' });
    await exportMarch.waitFor({ timeout: 7000 });
    assert(since(t1) >= 3800, `report ready too early (${since(t1)} ms)`);
    await exportMarch.click();
    await page.getByText('Relatório de Março exportado').waitFor();
  },
  'drag-drop': async () => {
    await page.goto(base + 'kanban.html');
    await page.dragAndDrop('#k1', '#list-done');
    await page.getByRole('list', { name: 'Concluído' }).getByText('Revisar contrato').waitFor();
  },
  'file-upload': async () => {
    const root = mkdtempSync(join(tmpdir(), 'n6-upload-'));
    try {
      execFileSync('python3', ['-c', 'import sys, pathlib, tasks_hard; tasks_hard.setup_upload(pathlib.Path(sys.argv[1]))', root], { cwd: here });
      await page.goto(base + 'upload.html');
      await page.getByRole('button', { name: 'Enviar' }).click();
      await page.getByRole('alert').filter({ hasText: 'Selecione o arquivo' }).waitFor();
      await page.getByLabel('Comprovante').setInputFiles(join(root, 'files', 'comprovante-outubro.pdf'));
      await page.getByRole('button', { name: 'Enviar' }).click();
      await page.getByText('Comprovante "comprovante-outubro.pdf" enviado').waitFor();
    } finally { rmSync(root, { recursive: true, force: true }); }
  },
  'multi-tab': async () => {
    await page.goto(base + 'boleto.html');
    const [popup] = await Promise.all([page.waitForEvent('popup'), page.getByRole('link', { name: 'Ver boleto' }).click()]);
    await popup.waitForLoadState();
    assert(await popup.title() === 'Boleto - Condomínio Solar', 'boleto opens in a new tab');
    const line = await popup.locator('#line').textContent();
    await popup.close();
    await page.getByLabel('Linha digitável').fill(line.slice(0, -1));
    await page.getByRole('button', { name: 'Pagar' }).click();
    await page.getByRole('alert').filter({ hasText: '47 números' }).waitFor();
    await page.getByLabel('Linha digitável').fill(line);
    await page.getByRole('button', { name: 'Pagar' }).click();
    await page.getByText('Pagamento agendado').waitFor();
  },
  'xorigin-iframe': async () => {
    await page.goto(base + 'bank.html');
    const f = page.frameLocator('iframe[title="Transferência Pix"]');
    await f.getByLabel('Chave Pix').fill('maria@example.com');
    const frame = page.frames().find(fr => fr.url().includes('transfer-frame'));
    assert(frame && frame.url().startsWith(`http://localhost:${port}/`), `frame must be cross-site (${frame && frame.url()})`);
    await f.getByLabel('Valor').fill('150,00');
    await f.getByLabel('Descrição').fill('Aluguel');
    await f.getByRole('button', { name: 'Continuar' }).click();
    await f.getByRole('button', { name: 'Confirmar transferência' }).click();
    await f.getByText('Transferência realizada').waitFor();
  },
  'recover-error': async () => {
    await page.goto(base + 'profile.html');
    await page.getByLabel('Telefone').fill('(11) 91234-5678');
    const save = page.getByRole('button', { name: 'Salvar alterações' });
    await save.click();
    await page.getByRole('alert').filter({ hasText: 'Não foi possível salvar. Tente novamente.' }).waitFor();
    assert(Object.keys(await state('recover-error')).length === 0, 'first save must store nothing');
    await save.click();
    await page.getByText('Alterações salvas').waitFor();
  },
  'async-autocomplete': async () => {
    await page.goto(base + 'shipping.html');
    await page.getByLabel('CEP').fill('88010-000');
    await page.getByLabel('Rua').fill('Rua Felipe Schmidt');
    await page.getByLabel('Número').fill('200');
    const city = page.getByRole('combobox', { name: 'Cidade' });
    const listbox = page.getByRole('listbox', { name: 'Cidades' });
    // Typing the full name without picking an option does not count.
    await city.fill('Florianópolis - SC');
    await page.getByRole('button', { name: 'Salvar endereço' }).click();
    await page.getByRole('alert').filter({ hasText: 'Selecione uma cidade' }).waitFor();
    await city.fill('');
    const t0 = Date.now();
    await city.pressSequentially('Florian');
    assert(!(await listbox.isVisible()), 'suggestions must not be instant');
    const option = page.getByRole('option', { name: 'Florianópolis - SC' });
    await option.waitFor({ timeout: 4000 });
    assert(since(t0) >= 1150, `suggestions too early (${since(t0)} ms)`);
    const names = await page.getByRole('option').allTextContents();
    assert(names.includes('Floriano - PI'), `decoy must be offered too (${names})`);
    await option.click();
    await page.getByRole('button', { name: 'Salvar endereço' }).click();
    await page.getByText('Endereço salvo').waitFor();
  },
};

for (const [task, flow] of Object.entries(flows)) {
  if (process.argv[3] && process.argv[3] !== task) continue;
  await reset();
  try { await flow(); await page.waitForTimeout(300); console.log('FLOW', task, 'done'); }
  catch (e) { console.log('FLOW', task, 'ERROR', e.message.split('\n')[0]); }
  console.log('STATE', task, JSON.stringify(await state(task)));
}
console.log('page errors:', errors.length ? errors : 'none');
await browser.close();
