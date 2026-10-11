// Project graph: joining two parts offers calls / pulls / pushes before "owns",
// and a part moves into another owner ("Move into…") or out of its owner
// ("Move out") after it was created (2026-10-10). Each action is checked on the server.
const { test, expect, openTab, posOf, still, dialog, shot, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'moves';
let ids;

const hasEdge = (g, from, kind, to) => g.edges.some((e) => e.from === from && e.kind === kind && e.to === to);
const select = async (page, sel) => { await page.evaluate((s) => { IterGraph.api.select(s); }, sel); await still(page); };
const menu = async (page, id, item) => {
  const p = await posOf(page, 'graph', id);
  await page.mouse.click(p.x, p.y, { button: 'right' });
  await expect(page.locator('.kit-menu')).toBeVisible();
  await page.locator('.kit-menu button', { hasText: item }).first().click();
};

test.describe('project graph: calls edges and moving parts', () => {
  // one seeded project, changed step by step: a failure must not re-seed it under the later tests
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async () => {
    const st = readState();
    const admin = new Api(st.base, st.adminToken);
    const eng = new Api(st.base, st.engineToken);
    ids = (await seedProject(admin, eng, PROJ, { conflict: false, pending: false, items: false })).ids;
  });

  test('drawing an edge between two parts offers "calls" first', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.webapp });
    await page.keyboard.press('c');
    await expect(page.locator('#g-banner')).toContainText('Connecting from');
    // tap the target through Cytoscape: a click at its centre can land on another box while the layout settles
    await page.evaluate((id) => IterGraph.api.cy.getElementById(id).emit('tap'), ids.gateway);
    await expect(dialog(page)).toBeVisible();
    const first = dialog(page).locator('.kit-radio').first();
    await expect(first).toContainText('Web app calls API gateway');
    await expect(dialog(page)).toContainText('pulls data from');
    await expect(dialog(page)).toContainText('pushes data to');
    await expect(dialog(page)).toContainText('owns');
    await shot(page, 'graph-edge-calls');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => hasEdge(await admin.graph(PROJ), ids.webapp, 'calls', ids.gateway)).toBe(true);
    const wa = await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.webapp}`);
    expect(wa.front.calls).toEqual([expect.stringContaining('gateway')]);
  });

  test('Move into…: a component moves to another container', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await menu(page, ids.checkout, 'Move into');
    await expect(dialog(page)).toBeVisible();
    await dialog(page).locator('#mv-filter').fill('ledger');
    await expect(dialog(page).locator('#mv-list')).toHaveValue(ids.ledger);
    await shot(page, 'graph-move-into');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => { const g = await admin.graph(PROJ); return [hasEdge(g, ids.ledger, 'codenodes', ids.checkout), hasEdge(g, ids.webapp, 'codenodes', ids.checkout)]; }).toEqual([true, false]);
  });

  test('Move out: a component leaves its container for the context above', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await menu(page, ids.settle, 'Move out');
    await expect.poll(async () => { const g = await admin.graph(PROJ); return [hasEdge(g, ids.payments, 'codenodes', ids.settle), hasEdge(g, ids.ledger, 'codenodes', ids.settle)]; }).toEqual([true, false]);
  });

  test("Cluster: dragging a part onto another part's box moves it inside, after a confirm", async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    const btn = page.locator('[data-layout=cluster]');
    if (await btn.count()) { await btn.first().click(); await still(page); }
    expect(await page.evaluate(() => IterGraph.api.layout)).toBe('cluster');
    const a = await posOf(page, 'graph', ids.gateway);
    const b = await posOf(page, 'graph', ids.postgres);
    await page.mouse.move(a.x, a.y);
    await page.mouse.down();
    await page.mouse.move(b.x, b.y, { steps: 12 });
    await page.mouse.up();
    await expect(dialog(page)).toContainText('inside');
    await shot(page, 'graph-drop-into');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => { const g = await admin.graph(PROJ); return [hasEdge(g, ids.postgres, 'codenodes', ids.gateway), hasEdge(g, ids.storefront, 'codenodes', ids.gateway)]; }).toEqual([true, false]);
  });

  test('a click on a part (no drag) never asks to move it', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    const p = await posOf(page, 'graph', ids.postgres);
    await page.mouse.click(p.x, p.y);
    await still(page);
    await expect(dialog(page)).toHaveCount(0);
  });

  test('renaming a part offers to rename its folder; yes moves it and every node file inside', async ({ page, admin }) => {
    const before = await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.webapp}`);
    expect(before.path.endsWith('/web_app/web_app.code.iter.md')).toBe(true);
    const want = before.path.replace('/web_app/web_app.code.iter.md', '/shop_front/shop_front.code.iter.md');
    const checkoutBefore = (await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.checkout}`)).path;
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.webapp });
    await page.keyboard.press('e');
    await expect(dialog(page)).toBeVisible();
    await dialog(page).locator('#kit-cf-0').fill('Shop front');
    await dialog(page).locator('.kit-primary').click();
    const ask = dialog(page).filter({ hasText: 'Rename its folder too?' });
    await expect(ask).toBeVisible();
    await expect(ask).toContainText('src/storefront/web_app/');
    await expect(ask).toContainText('src/storefront/shop_front/');
    await shot(page, 'graph-rename-folder');
    await ask.locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    await expect.poll(async () => (await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.webapp}`)).path).toBe(want);
    const after = await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.webapp}`);
    expect(after.name).toBe('Shop front');
    expect((await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.checkout}`)).path).toBe(checkoutBefore.replace('/web_app/', '/shop_front/'));
  });

  test('renaming and declining keeps the folder and file names', async ({ page, admin }) => {
    const before = (await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.ledger}`)).path;
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.ledger });
    await page.keyboard.press('e');
    await dialog(page).locator('#kit-cf-0').fill('Ledger API');
    await dialog(page).locator('.kit-primary').click();
    const ask = dialog(page).filter({ hasText: 'Rename its folder too?' });
    await expect(ask).toBeVisible();
    await ask.locator('.kit-ghost', { hasText: 'Cancel' }).click();
    await expect.poll(async () => (await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.ledger}`)).name).toBe('Ledger API');
    expect((await admin.get(`/api/projects/${PROJ}/graph/nodes/${ids.ledger}`)).path).toBe(before);
  });

  test('a new part still hangs from its parent by "owns"', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.storefront });
    await page.keyboard.press('n');
    await expect(dialog(page)).toBeVisible();
    await dialog(page).locator('input[name=nt][value=container]').check().catch(() => {});
    await expect(dialog(page).locator('#nn-kind option').first()).toHaveAttribute('value', 'codenodes');
    await page.keyboard.press('Escape');
  });
});
