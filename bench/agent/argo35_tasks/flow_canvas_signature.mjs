
export const TASK_ID="a35-canvas-signature";
export async function flow({page,context,base,root,assert}){
await page.goto(base+"canvas_signature.html");
const r=await page.locator('canvas').boundingBox();await page.mouse.move(r.x+40,r.y+100);await page.mouse.down();for(let i=1;i<=40;i++)await page.mouse.move(r.x+40+i*10,r.y+100+Math.sin(i)*35);await page.mouse.up();await page.getByLabel('Li e concordo com os termos').check();await page.getByRole('button',{name:'Assinar contrato',exact:true}).click();
}
