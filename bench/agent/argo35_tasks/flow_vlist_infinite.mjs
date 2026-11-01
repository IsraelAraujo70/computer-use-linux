export const TASK_ID='a35-vlist-infinite';
export async function flow({page,base}){await page.goto(base+'vlist_infinite.html'); for(let i=0;i<11;i++){await page.locator('#view').hover(); await page.mouse.wheel(0,100000); await page.waitForTimeout(850);} await page.getByRole('button',{name:'Curtir publicação #237',exact:true}).click(); }
