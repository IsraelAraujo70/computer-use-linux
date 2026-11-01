export const TASK_ID = 'a35-tabs-compare';
export async function flow({page, context, base, assert}) {
await page.goto(base+'tabs_compare.html');
for(const name of ['Atlas','Boreal','Cedro']){const opened=page.waitForEvent('popup');await page.getByRole('link',{name,exact:true}).click();const tab=await opened;await tab.getByText('Código do fornecedor:',{exact:false}).waitFor();await tab.close();}await page.getByLabel('Código do fornecedor').fill('BOR-42');await page.getByText('Adicionar ao pedido',{exact:true}).click();
}
