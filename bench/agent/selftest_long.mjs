// Drives each long task's happy path headlessly (long_tasks/flow_<name>.mjs) and prints
// the state each page posted, for check_selftest_long.py.
// PLAYWRIGHT_CORE=/path/to/playwright-core CHROME=/path/to/chrome node selftest_long.mjs <port> [module]
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE);
const here = dirname(fileURLToPath(import.meta.url));
const port = process.argv[2];
const MODULES = ['onboarding', 'expenses', 'triage', 'crm', 'checkout'];
const base = `http://127.0.0.1:${port}/apps/long/`;
const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
const context = await browser.newContext({ viewport: { width: 1900, height: 950 } });
const errors = [];
const assert = (cond, msg) => { if (!cond) throw new Error('assertion failed: ' + msg); };
for (const name of MODULES) {
  if (process.argv[3] && process.argv[3] !== name) continue;
  const page = await context.newPage();
  page.on('pageerror', e => errors.push(`${name}: ${e.message}`));
  await fetch(`http://127.0.0.1:${port}/api/reset`, { method: 'POST', body: '{}' });
  const { flow, TASK_ID } = await import(pathToFileURL(join(here, 'long_tasks', `flow_${name}.mjs`)).href);
  try { await flow({ page, base, assert }); await page.waitForTimeout(300); console.log('FLOW', TASK_ID, 'done'); }
  catch (e) { console.log('FLOW', TASK_ID, 'ERROR', e.message.split('\n')[0]); }
  const state = await (await fetch(`http://127.0.0.1:${port}/api/state/${TASK_ID}`)).json();
  console.log('STATE', TASK_ID, JSON.stringify(state));
  await page.close();
}
console.log('page errors:', errors.length ? errors : 'none');
await browser.close();
