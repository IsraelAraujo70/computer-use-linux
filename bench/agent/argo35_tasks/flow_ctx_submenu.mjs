export const TASK_ID = 'a35-ctx-submenu';
export async function flow({page, base}) {
try { await page.goto(base + "ctx_submenu.html");
await page.getByText('Fornecedor XPTO — Cotação de servidores · Entrada',{exact:true}).click({button:'right'});await page.locator('.parent').filter({hasText:'Mover para'}).first().hover();await page.locator('.parent').filter({hasText:'Projetos ▸'}).last().hover();await page.getByText('Projeto Aurora',{exact:true}).click();
} catch(e) { throw new Error(e.message.replaceAll('\n',' | ')); }
}
