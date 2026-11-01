export const TASK_ID='a35-vlist-grid';
export async function flow({page,base}){await page.goto(base+'vlist_grid.html'); await page.locator('#view').hover(); await page.mouse.wheel(3000,112284); await page.locator('[aria-label="SKU-03120:Estoque"]').dblclick(); await page.getByLabel('Editar SKU-03120:Estoque',{exact:true}).fill('42'); await page.keyboard.press('Enter'); }
