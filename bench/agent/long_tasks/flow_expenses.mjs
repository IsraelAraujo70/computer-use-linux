export const TASK_ID = 'long-expenses';
export async function flow({ page, base, assert }) {
  await page.goto(base + 'expenses.html');
  await page.getByLabel('Título do relatório').fill('Visita cliente Recife - outubro');
  await page.getByLabel('Centro de custo').selectOption('Comercial');
  await page.getByLabel('Projeto', {exact:true}).selectOption('Aurora');
  const rows = [["06/10/2026", "Transporte", "Táxi aeroporto", 8750], ["06/10/2026", "Hospedagem", "Hotel Boa Viagem 2 diárias", 64000], ["07/10/2026", "Alimentação", "Almoço com cliente", 18230], ["07/10/2026", "Transporte", "Uber reunião", 3490], ["08/10/2026", "Material de escritório", "Impressões proposta", 4500]];
  for (const [date,category,description,amount] of rows) {
    await page.getByRole('button', {name:'Adicionar despesa'}).click();
    if (description === rows[0][2]) {
      await page.getByRole('button', {name:'Salvar despesa'}).click();
      assert(await page.getByText('Informe uma data válida.').isVisible(), 'validação obrigatória');
    }
    await page.getByLabel('Data', {exact:true}).fill(date);
    await page.getByLabel('Categoria', {exact:true}).selectOption(category);
    await page.getByLabel('Descrição', {exact:true}).fill(description);
    await page.getByLabel('Valor (R$)', {exact:true}).fill((amount/100).toFixed(2).replace('.',','));
    await page.getByRole('button', {name:'Salvar despesa'}).click();
  }
  await page.getByRole('button', {name:'Enviar para aprovação'}).click();
  await page.getByRole('button', {name:'Enviar',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Relatório enviado'}).waitFor();
}
