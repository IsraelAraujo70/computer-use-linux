export const TASK_ID = 'a35-ctx-rename';
export async function flow({page, base}) {
try { await page.goto(base + "ctx_rename.html");
await page.getByText('rascunho-v3.docx',{exact:true}).click({button:'right'});await page.getByText('Renomear',{exact:true}).click();await page.getByLabel('Nome do arquivo').fill('proposta-final.docx');await page.keyboard.press('Enter');
} catch(e) { throw new Error(e.message.replaceAll('\n',' | ')); }
}
