// Drives each argo35 task's happy path headlessly (argo35_tasks/flow_<module>.mjs) and prints the
// state each page posted. Run through check_selftest_argo35.py, which starts the server, runs each
// module's setup() into ARGO_ROOT and feeds these STATE lines to the checkers.
// PLAYWRIGHT_CORE=... CHROME=... ARGO_ROOT=... node selftest_argo35.mjs <port> <module>...
import { createRequire } from 'node:module';
import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE);
const here = dirname(fileURLToPath(import.meta.url));
const [port, ...modules] = process.argv.slice(2);
const root = process.env.ARGO_ROOT;
const base = `http://127.0.0.1:${port}/apps/argo35/`;
mkdirSync(join(root, 'downloads'), { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
const errors = [];
const assert = (cond, msg) => { if (!cond) throw new Error('assertion failed: ' + msg); };
for (const name of modules) {
  // A fresh context per task: popups, tabs and downloads of one task never leak into the next.
  const context = await browser.newContext({ viewport: { width: 1900, height: 950 }, acceptDownloads: true });
  const page = await context.newPage();
  context.on('page', p => p.on('pageerror', e => errors.push(`${name} (popup): ${e.message}`)));
  page.on('pageerror', e => errors.push(`${name}: ${e.message}`));
  await fetch(`http://127.0.0.1:${port}/api/reset`, { method: 'POST', body: '{}' });
  let taskId = name;
  try {
    const mod = await import(pathToFileURL(join(here, 'argo35_tasks', `flow_${name}.mjs`)).href);
    taskId = mod.TASK_ID;
    await mod.flow({ page, context, base, root, assert });
    await page.waitForTimeout(400);
    console.log('FLOW', name, taskId, 'done');
  } catch (e) {
    console.log('FLOW', name, taskId, 'ERROR', e.message.split('\n')[0]);
  }
  const state = await (await fetch(`http://127.0.0.1:${port}/api/state/${taskId}`)).json();
  console.log('STATE', name, taskId, JSON.stringify(state));
  await context.close();
}
console.log('PAGEERRORS', JSON.stringify(errors));
await browser.close();
