export const TASK_ID = 'a35-dnd-slider';
export async function flow({page,base}) {
 await page.goto(base+'dnd_slider.html');
for(const [name,value] of [['Volume',35],['Graves',80]]){const r=await page.locator('[data-name="'+name+'"]').boundingBox();await page.mouse.click(r.x+4*value,r.y+6);}await page.getByRole('button',{name:'Aplicar',exact:true}).click();
}
