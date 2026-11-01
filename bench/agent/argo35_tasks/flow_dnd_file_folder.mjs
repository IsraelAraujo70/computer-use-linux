export const TASK_ID = 'a35-dnd-file-folder';
export async function flow({page,base}) {
 await page.goto(base+'dnd_file_folder.html');
 for(const name of ['contrato-assinado.pdf','nota-fiscal-0931.xml']) {
 const s=await page.locator('[data-name="'+name+'"]').boundingBox(),t=await page.locator('[data-folder="Financeiro 2026"]').boundingBox();
 await page.mouse.move(s.x+s.width/2,s.y+s.height/2);await page.mouse.down();await page.mouse.move(s.x+s.width/2+12,s.y+s.height/2,{steps:5});await page.mouse.move(t.x+t.width/2,t.y+t.height/2,{steps:30});await page.mouse.move(t.x+t.width/2+2,t.y+t.height/2,{steps:3});await page.mouse.up();
 }
}
