export const TASK_ID='a35-vlist-10k';
export async function flow({page,base}){await page.goto(base+'vlist_10k.html'); await page.locator('#view').hover(); await page.mouse.wheel(0,143640); await page.getByRole('button',{name:'Aprovar registro #4000',exact:true}).click(); }
