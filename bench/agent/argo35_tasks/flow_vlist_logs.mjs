export const TASK_ID='a35-vlist-logs';
export async function flow({page,base}){await page.goto(base+'vlist_logs.html'); await page.locator('#view').hover(); await page.mouse.wheel(0,570200); await page.getByRole('button',{name:'Resolver linha 6200',exact:true}).click(); }
