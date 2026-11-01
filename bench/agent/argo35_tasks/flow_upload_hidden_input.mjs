import fs from 'node:fs/promises';import path from 'node:path';
export const TASK_ID="a35-upload-hidden-input";
export async function flow({page,context,base,root,assert}){
await page.goto(base+"upload_hidden_input.html");
await page.locator('input[type=file]').setInputFiles(['rg-frente.jpg', 'rg-verso.jpg'].map(n=>path.join(root,'files',n)));await page.getByText('rg-verso.jpg',{exact:false}).waitFor();await page.getByRole('button',{name:'Enviar documentos'}).click();await page.getByText('Envio concluído').waitFor();
}
