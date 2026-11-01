export const TASK_ID='a35-canvas-connect';
export async function flow({page,base}){
await page.goto(base+'canvas_connect.html');
const r=await page.locator('canvas').boundingBox();
const move=async(x,y)=>page.mouse.move(r.x+x*r.width/1200,r.y+y*r.height/700);
await page.getByRole('button',{name:'Conector',exact:true}).click();
await move(190,160);
await page.mouse.down();await move(577,320);await page.mouse.up();
await move(577,320);
await page.mouse.down();await move(981,150);await page.mouse.up();
await page.getByRole("status").waitFor();
}
