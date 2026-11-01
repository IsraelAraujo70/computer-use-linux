export const TASK_ID = 'a35-hover-tooltip';
export async function flow({page, context, base, assert}) {
await page.goto(base+'hover_tooltip.html');
const cards=page.locator('#plans article');for(let i=0;i<4;i++){await cards.nth(i).getByLabel('Informações sobre Suporte',{exact:true}).hover();if(await page.getByRole('tooltip').textContent()==='Suporte 24h por telefone'){await cards.nth(i).getByRole('button',{name:'Assinar'}).click();break;}}
}
