import fs from 'node:fs/promises';import path from 'node:path';
export const TASK_ID="a35-term-code";
export async function flow({page,context,base,root,assert}){
await page.goto(base+"term_code.html");
const code=(await fs.readFile(path.join(root,'term','codigo-verificacao.txt'),'utf8')).trim();await page.getByLabel('Código de verificação').fill(code);await page.getByRole('button',{name:'Verificar',exact:true}).click();
}
