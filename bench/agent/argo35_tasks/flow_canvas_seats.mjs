export const TASK_ID='a35-canvas-seats';
export async function flow({page,base}){
await page.goto(base+'canvas_seats.html');
const r=await page.locator('canvas').boundingBox();
const move=async(x,y)=>page.mouse.move(r.x+x*r.width/1200,r.y+y*r.height/700);
await move(599,386);
await page.mouse.down();await page.mouse.up();
await move(664,386);
await page.mouse.down();await page.mouse.up();
await page.getByRole('button',{name:'Confirmar assentos',exact:true}).click();
await page.getByRole("status").waitFor();
}
