export const TASK_ID = 'a35-kbd-gmail';
export async function flow({page, base}) {
await page.goto(base + "kbd_gmail.html");
for(let i=0;i<15;i++){if([1,4,8,12].includes(i))await page.keyboard.press('x');if(i===6)await page.keyboard.press('s');if(i<14)await page.keyboard.press('j');}await page.keyboard.press('e');
}
