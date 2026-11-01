import fs from 'node:fs/promises';import path from 'node:path';
export const TASK_ID="a35-download-csv";
export async function flow({page,context,base,root,assert}){
await page.goto(base+"download_csv.html");
await page.getByLabel('Período').selectOption('2026-10');await page.getByLabel('Região').selectOption({label:'Sul'});const waiting=page.waitForEvent('download');await page.getByRole('button',{name:'Baixar CSV'}).click();const d=await waiting;await fs.mkdir(path.join(root,'downloads'),{recursive:true});await d.saveAs(path.join(root,'downloads',d.suggestedFilename()));
}
