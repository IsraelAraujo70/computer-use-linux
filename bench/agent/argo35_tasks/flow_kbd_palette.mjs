export const TASK_ID = 'a35-kbd-palette';
export async function flow({page, base}) {
await page.goto(base + "kbd_palette.html");
for(const command of ['Ativar quebra de linha automática','Solarizado claro','Indentação: 2 espaços']){await page.keyboard.press('Control+k');await page.getByRole('combobox').fill(command);await page.keyboard.press('Enter');}
}
