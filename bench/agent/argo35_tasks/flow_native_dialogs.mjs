export const TASK_ID = 'a35-native-dialogs';
export async function flow({page, context, base, assert}) {
await page.goto(base+'native_dialogs.html');
page.on('dialog',async d=>{if(d.type()==='prompt')await d.accept('Projeto Legado');else await d.accept();});await page.getByRole('button',{name:'Excluir Projeto Legado',exact:true}).click();await page.getByRole('button',{name:'Excluir Projeto Legado',exact:true}).waitFor({state:'detached'});
}
