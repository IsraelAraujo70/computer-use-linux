export const TASK_ID = 'a35-kbd-only-otp';
export async function flow({page, base}) {
await page.goto(base + "kbd_only_otp.html");
await page.getByLabel('Dígito 1',{exact:true}).focus();await page.keyboard.type('482913');await page.getByLabel('PIN do cartão').focus();await page.keyboard.type('7350');await page.getByRole('button',{name:'Confirmar'}).click();
}
