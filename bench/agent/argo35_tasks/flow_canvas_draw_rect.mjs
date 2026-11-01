export const TASK_ID='a35-canvas-draw-rect';
export async function flow({page,base}){
await page.goto(base+'canvas_draw_rect.html');
const r=await page.locator('canvas').boundingBox();
const move=async(x,y)=>page.mouse.move(r.x+x*r.width/1200,r.y+y*r.height/700);
await page.getByRole('button',{name:'Retângulo',exact:true}).click();
await move(320,180);
await page.mouse.down();await move(740,420);await page.mouse.up();
await page.getByRole('button',{name:'Salvar',exact:true}).click();
await page.getByRole("status").waitFor();
}
