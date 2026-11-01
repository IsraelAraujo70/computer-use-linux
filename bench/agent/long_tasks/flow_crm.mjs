export const TASK_ID = 'long-crm';
export async function flow({page,base,assert}) {
 await page.goto(base+'crm.html');
 for (const [i,name,value] of [[0,'Ana Ribeiro','12.500,00'],[1,'Bruno Teixeira','8.900,00'],[2,'Carla Menezes','23.000,00'],[3,'Diego Antunes','4.750,00'],[4,'Elisa Prado','15.300,00']]) {
  if(i===3) await page.getByRole('button',{name:'Próxima página',exact:true}).click();
  await page.getByRole('button',{name:'Editar '+name,exact:true}).click();
  await page.getByLabel('Status',{exact:true}).selectOption('Cliente');
  if(i===0){await page.getByRole('button',{name:'Salvar',exact:true}).click();await page.getByRole('alert').filter({hasText:'maior que zero'}).waitFor();assert(await page.getByRole('dialog').isVisible(),'validation blocks save');}
  await page.getByLabel('Valor do contrato',{exact:true}).fill(value);
  await page.getByRole('button',{name:'Salvar',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Lead salvo'}).waitFor();
 }
 await page.getByLabel('Buscar lead').fill('Rafael Souza');
 await page.getByRole('button',{name:'Excluir Rafael Souza (duplicado)',exact:true}).click();
 await page.getByRole('dialog').getByRole('button',{name:'Excluir',exact:true}).click();
 await page.getByRole('status').filter({hasText:'Lead excluído'}).waitFor();
}
