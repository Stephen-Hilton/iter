#!/usr/bin/env node
// deployed_smoke.js — a browser check of a DEPLOYED iter5 (default http://127.0.0.1:8400):
// signs in as admin, creates a throwaway project with a context node and a multi-section
// requirements file (through the API), then in Chromium opens every tab (no console errors),
// the project graph, the requirements table on the file, and the req nodes; screenshots go to
// screenshots/deployed-*.png. The throwaway project is deleted at the end.
//   node deployed_smoke.js [--url http://host:8400]
// Admin password: ITER_ADMIN_PASSWORD, else read from ../../../.env.
'use strict';
const fs = require('fs');
const path = require('path');
const { chromium } = require('@playwright/test');
const { Api } = require('./lib/api');

const args = process.argv.slice(2);
const BASE = (args[0] === '--url' && args[1] ? args[1] : 'http://127.0.0.1:8400').replace(/\/$/, '');
const SHOTS = path.join(__dirname, 'screenshots');

function adminPassword() {
  if (process.env.ITER_ADMIN_PASSWORD) return process.env.ITER_ADMIN_PASSWORD;
  const env = path.resolve(__dirname, '../../../.env');
  const line = fs.readFileSync(env, 'utf8').split('\n').filter((l) => l.startsWith('ITER_ADMIN_PASSWORD=')).pop() || '';
  return line.slice('ITER_ADMIN_PASSWORD='.length).replace(/^["']|["']$/g, '');
}
function chromiumPath() {
  return require('./playwright.config.js').use.launchOptions.executablePath;
}

let pass = 0; let fail = 0; const failed = [];
const ok = (m) => { pass++; console.log(`\x1b[32mPASS\x1b[0m ${m}`); };
const bad = (m) => { fail++; failed.push(m); console.log(`\x1b[31mFAIL\x1b[0m ${m}`); };
async function check(name, fn) { try { const r = await fn(); if (r === false) bad(name); else ok(name); } catch (e) { bad(`${name}: ${e.message.split('\n')[0]}`); } }

(async () => {
  const PROJ = `pwsmoke5_${Math.floor(Date.now() / 1000)}`;
  const P = `/api/projects/${PROJ}`;
  const { api: admin, token } = await Api.login(BASE, 'admin', adminPassword());
  ok(`admin login on ${BASE}`);
  let browser;
  try {
    // ---- data through the API
    await admin.put(P, { state: 'Stopped', desc: 'deployed browser smoke project' });
    const g0 = await admin.graph(PROJ);
    const proj = g0.nodes.find((n) => n.nodetype === 'project');
    const ctx = (await admin.post(`${P}/graph/nodes`, { nodetype: 'code', level: 'context', name: 'Checkout', desc: 'takes orders', attach_to: proj.id, attach_kind: 'codenodes' })).node;
    const r1 = await admin.post(`${P}/graph/reqs`, { node: ctx.id, type: 'techreq', key: 'SMK-T-001', title: 'Orders are idempotent', text: 'A repeated order request with the same key creates one order.', status: 'agreed' });
    await admin.post(`${P}/graph/reqs`, { file: r1.file.id, key: 'SMK-T-002', title: 'Prices are integers', text: 'Amounts are stored in minor units.', status: 'draft' });
    await admin.post(`${P}/graph/reqs`, { file: r1.file.id, key: 'SMK-T-003', title: 'Every order is logged', text: 'One structured log line per order.', status: 'done' });
    const g1 = await admin.graph(PROJ);
    const reqs = g1.nodes.filter((n) => n.nodetype === 'req' && n.file === r1.file.id);
    if (reqs.length === 3) ok('server derived 3 req nodes from one techreq file'); else bad(`server derived ${reqs.length} req nodes (want 3)`);

    // ---- browser
    browser = await chromium.launch({ executablePath: chromiumPath() });
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const consoleErrors = [];
    page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') consoleErrors.push(m.text()); });
    page.on('pageerror', (e) => consoleErrors.push(String(e)));
    await page.addInitScript(([t]) => { localStorage.setItem('iter5.token', t); localStorage.setItem('iter5.user', 'admin'); localStorage.setItem('iter5.role', 'admin'); }, [token]);
    for (const tab of ['queue', 'graph', 'rag', 'settings', 'intro']) {
      await check(`tab ${tab} loads`, async () => {
        await page.goto(`${BASE}/#tab=${tab}&p=${PROJ}`);
        await page.waitForLoadState('networkidle');
        await page.waitForTimeout(800);
        await page.screenshot({ path: path.join(SHOTS, `deployed-${tab}.png`) });
      });
    }
    await check('project graph draws the context and the requirements file', async () => {
      await page.goto(`${BASE}/#tab=graph&p=${PROJ}`);
      await page.waitForFunction(() => window.IterGraph && IterGraph.api && IterGraph.api.cy && IterGraph.api.cy.nodes().length > 0, null, { timeout: 15000 });
      const ids = await page.evaluate(() => IterGraph.api.cy.nodes().map((n) => n.id()));
      return ids.includes(ctx.id) && ids.includes(r1.file.id);
    });
    await check('requirements table shows the 3 requirements', async () => {
      await page.evaluate((id) => IterGraph.api.select({ kind: 'node', id }), r1.file.id);
      await page.waitForSelector('#g-detail tr.g-rrow', { timeout: 10000 });
      const n = await page.locator('#g-detail tr.g-rrow').count();
      await page.screenshot({ path: path.join(SHOTS, 'deployed-req-table.png') });
      if (n !== 3) throw new Error(`${n} rows`);
    });
    await check('req nodes appear when the req type is shown', async () => {
      await page.click('#g-chips button[data-type=req]');
      await page.waitForTimeout(800);
      const shown = await page.evaluate((ids) => ids.filter((i) => { const n = IterGraph.api.cy.getElementById(i); return n.nonempty() && n.visible(); }).length, reqs.map((r) => r.id));
      await page.screenshot({ path: path.join(SHOTS, 'deployed-req-nodes.png') });
      if (shown !== 3) throw new Error(`${shown} visible`);
    });
    if (consoleErrors.length === 0) ok('no console errors or warnings'); else bad(`console: ${consoleErrors.slice(0, 3).join(' | ')}`);
  } catch (e) {
    bad(`unexpected: ${e.message}`);
  } finally {
    if (browser) await browser.close();
    await admin.call('DELETE', P, undefined, [404]).then(() => ok('throwaway project deleted')).catch((e) => bad(`cleanup: ${e.message}`));
  }
  console.log(`\ndeployed browser smoke: ${pass} passed, ${fail} failed`);
  if (fail) { failed.forEach((f) => console.log('  ' + f)); process.exit(1); }
})();
