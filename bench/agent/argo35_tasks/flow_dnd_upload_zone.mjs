import fs from 'node:fs/promises';import path from 'node:path';
export const TASK_ID="a35-dnd-upload-zone";
export async function flow({page,context,base,root,assert}){
await page.goto(base+"dnd_upload_zone.html");
const names=['print-erro.png', 'log-servidor.txt'];const payload=await Promise.all(names.map(async name=>({name,bytes:Array.from(await fs.readFile(path.join(root,'files',name)))})));const dt=await page.evaluateHandle(items=>{const d=new DataTransfer();for(const f of items)d.items.add(new File([new Uint8Array(f.bytes)],f.name));return d},payload);await page.locator('#drop').dispatchEvent('drop',{dataTransfer:dt});await page.getByText(names[1],{exact:false}).waitFor();await page.getByRole('button',{name:'Enviar anexos'}).click();await page.getByText('Envio concluído').waitFor();
}
