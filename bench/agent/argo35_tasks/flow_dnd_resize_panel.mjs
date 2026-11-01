export const TASK_ID = 'a35-dnd-resize-panel';
export async function flow({page,base}) {
 await page.goto(base+'dnd_resize_panel.html');
for(const [axis,delta] of [['width',320],['height',-120]]){const r=await page.locator('[data-axis='+axis+']').boundingBox();await page.mouse.move(r.x+3,r.y+3);await page.mouse.down();await page.mouse.move(r.x+3+(axis==='width'?delta:0),r.y+3+(axis==='height'?delta:0),{steps:20});await page.mouse.up();}
}
