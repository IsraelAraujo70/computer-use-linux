export const TASK_ID = 'a35-dialog-nested';
export async function flow({page, context, base, assert}) {
await page.goto(base+'dialog_nested.html');
await page.getByText('Recusar',{exact:true}).click();await page.getByRole('button',{name:'Configurações',exact:true}).click();await page.getByLabel('Fuso horário').selectOption('America/Manaus');await page.getByText('Salvar',{exact:true}).click();await page.getByRole('button',{name:'Confirmar',exact:true}).click();
}
