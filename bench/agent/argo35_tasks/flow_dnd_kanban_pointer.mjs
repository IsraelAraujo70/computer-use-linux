export const TASK_ID = 'a35-dnd-kanban-pointer';
export async function flow({page,base}) {
 await page.goto(base+'dnd_kanban_pointer.html');
async function drag(source,target,dy=0){const s=await page.locator(source).boundingBox(),t=await page.locator(target).boundingBox();await page.mouse.move(s.x+s.width/2,s.y+s.height/2);await page.mouse.down();await page.mouse.move(s.x+s.width/2+8,s.y+s.height/2,{steps:3});await page.mouse.move(t.x+t.width/2,t.y+dy,{steps:20});await page.mouse.up();}
await drag('[data-id=sso]','[data-col=done]',65);await drag('[data-id=e2e]','[data-col=doing]',65);
}
