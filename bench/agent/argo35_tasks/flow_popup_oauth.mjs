export const TASK_ID = 'a35-popup-oauth';
export async function flow({page, context, base, assert}) {
await page.goto(base+'popup_oauth.html');
const opened=page.waitForEvent('popup');await page.getByText('Conectar Google Drive',{exact:true}).click();const popup=await opened;await popup.getByLabel('israel@empresa.com',{exact:true}).check();await popup.getByText('Permitir',{exact:true}).click();await page.getByText('Conectado como israel@empresa.com',{exact:true}).waitFor();await page.getByText('Salvar integrações',{exact:true}).click();
}
