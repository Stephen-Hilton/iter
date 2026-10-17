// Shared fixtures: the seeded server (state), an admin API client, and a page
// that is already signed in (token in localStorage, as the login form leaves it)
// and fails the test on any console error/warning or uncaught page error.
'use strict';
const base = require('@playwright/test');
const fs = require('fs');
const path = require('path');
const { Api } = require('./api');

const STATE = path.join(__dirname, '..', '.state.json');
const SHOTS = path.join(__dirname, '..', 'screenshots');
const readState = () => JSON.parse(fs.readFileSync(STATE, 'utf8'));

const test = base.test.extend({
  state: async ({}, use) => { await use(readState()); },
  admin: async ({ state }, use) => { await use(new Api(state.base, state.adminToken)); },
  engine: async ({ state }, use) => { await use(new Api(state.base, state.engineToken)); },
  baseURL: async ({ state }, use) => { await use(state.base); },
  /** set to false in a describe (test.use({ signedIn: false })) for the login page */
  signedIn: [true, { option: true }],
  /** messages a test expects (regexes); anything else on the console fails the test */
  allowConsole: [[], { option: true }],
  consoleErrors: async ({}, use) => { await use([]); },
  page: async ({ page, state, signedIn, consoleErrors, allowConsole }, use) => {
    if (signedIn) {
      await page.addInitScript(([t]) => {
        try {
          if (!sessionStorage.getItem('pw.loggedout')) {
            localStorage.setItem('iter5.token', t); localStorage.setItem('iter5.user', 'admin'); localStorage.setItem('iter5.role', 'admin');
          }
        } catch (e) { /* storage blocked */ }
      }, [state.adminToken]);
    }
    page.on('console', (m) => {
      if (m.type() !== 'error' && m.type() !== 'warning') return;
      const text = `[${m.type()}] ${m.text()}`;
      if (allowConsole.some((re) => re.test(text))) return;
      consoleErrors.push(text);
    });
    page.on('pageerror', (e) => consoleErrors.push('[pageerror] ' + e.message));
    await use(page);
    base.expect(consoleErrors, 'no console errors or warnings').toEqual([]);
  },
});

/** Open a tab (queue | graph | settings | intro | rag) for a project and wait until it has drawn. */
async function openTab(page, tab, project) {
  await page.goto(`/#tab=${tab}${project ? '&p=' + encodeURIComponent(project) : ''}`);
  await page.waitForSelector('#top', { state: 'visible' });
  if (tab === 'graph') {
    await page.waitForSelector('#g-cy canvas');
    await page.waitForFunction((p) => window.IterGraph && IterGraph.api && IterGraph.api.model && IterGraph.api.model.G && IterGraph.api.model.G.nodes.length > 0 && document.querySelector('#g-editbar [data-ed]'), project);
    await page.waitForTimeout(500); // layout animation
  } else if (tab === 'settings') {
    await page.waitForSelector('#s-cy canvas');
    await page.waitForFunction(() => window.IterSettings && IterSettings.cy && IterSettings.cy.nodes().length > 0);
    await page.waitForTimeout(500);
  } else if (tab === 'queue') {
    await page.waitForSelector('#items .tile');
  } else if (tab === 'rag') {
    await page.waitForSelector('#rag-status .rag-stats');
  } else if (tab === 'intro') {
    await page.waitForSelector('#tab-intro .intro-slide, #tab-intro section, #tab-intro h1, #tab-intro h2');
    await page.waitForTimeout(300);
  } else {
    await page.waitForTimeout(600);
  }
}

/** Screen position of a Cytoscape element (graph: IterGraph.api.cy, settings: IterSettings.cy). */
async function posOf(page, which, id) {
  return page.evaluate(([which, id]) => {
    const cy = which === 'settings' ? IterSettings.cy : IterGraph.api.cy;
    const el = cy.getElementById(id);
    if (el.empty()) return null;
    const box = cy.container().getBoundingClientRect();
    const p = el.isEdge() ? el.renderedMidpoint() : el.renderedPosition();
    return { x: box.left + p.x, y: box.top + p.y };
  }, [which, id]);
}

/** Wait until the drawing stops moving (focus/fit animations, layout). */
async function still(page, which) {
  await page.waitForFunction((w) => { const cy = w === 'settings' ? IterSettings.cy : IterGraph.api.cy; return cy && !cy.animated() && !cy.elements().some((e) => e.animated()); }, which || 'graph');
  await page.waitForTimeout(150);
}

/** Drag the selected edge's source/target handle onto a node. */
async function dragHandle(page, which, end, targetId) {
  await still(page, which);
  const h = await posOf(page, which, end === 'source' ? '__kit_ep_s' : '__kit_ep_t');
  const t = await posOf(page, which, targetId);
  if (!h || !t) throw new Error(`no ${!h ? 'handle' : 'target'} on screen`);
  await page.mouse.move(h.x, h.y);
  await page.mouse.down();
  await page.mouse.move((h.x + t.x) / 2, (h.y + t.y) / 2, { steps: 8 });
  await page.mouse.move(t.x, t.y, { steps: 8 });
  await page.mouse.up();
}

const dialog = (page) => page.locator('dialog.kit-dlg[open]');

async function shot(page, name, opts) {
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.screenshot(Object.assign({ path: path.join(SHOTS, name + '.png') }, opts || {}));
}

module.exports = { test, expect: base.expect, openTab, posOf, dragHandle, still, dialog, shot, readState, Api };
