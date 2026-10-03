// Project graph, edit side — each action is done in the page and checked on the
// server: configure a node and an edge, create a node attached to a context,
// draw an edge, remove an edge (with a reason), drag an edge end onto another
// node, copy/paste an edge, delete a node (with a reason).
const { test, expect, openTab, posOf, dragHandle, still, dialog, shot, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'edits';
const P = `/api/projects/${PROJ}`;
let ids;

const hasEdge = (g, from, kind, to) => g.edges.some((e) => e.from === from && e.kind === kind && e.to === to);
const select = async (page, sel) => { await page.evaluate((s) => { IterGraph.api.select(s); }, sel); await still(page); };
const selectEdge = async (page, from, kind, to) => { await page.evaluate((id) => { IterGraph.api.focusEdgeId(id); }, `e:${from}|${kind}|${to}`); await still(page); };

test.describe('project graph: editing', () => {
  test.beforeAll(async () => {
    const st = readState();
    const admin = new Api(st.base, st.adminToken);
    const eng = new Api(st.base, st.engineToken);
    const seeded = await seedProject(admin, eng, PROJ, { conflict: false, pending: false, items: false });
    ids = seeded.ids;
  });

  test('configure a node: edit desc → PATCH reflected', async ({ page, admin }) => {
    const before = (await admin.get(`${P}/graph/nodes/${ids.ledger}`));
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.ledger });
    await page.keyboard.press('e');
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page).locator('.kit-meta')).toContainText('Container');
    await shot(page, 'graph-configure-node');
    const req = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().endsWith(`/graph/nodes/${ids.ledger}`));
    await dialog(page).locator('#kit-cf-1').fill('Records every payment as two balanced ledger entries; refunds too.');
    await dialog(page).locator('.kit-primary').click();
    const body = JSON.parse((await req).postData());
    expect(body).toMatchObject({ desc: 'Records every payment as two balanced ledger entries; refunds too.', expect_version: before.node_version });
    await expect(dialog(page)).toHaveCount(0);
    const after = await admin.get(`${P}/graph/nodes/${ids.ledger}`);
    expect(after.desc).toBe('Records every payment as two balanced ledger entries; refunds too.');
    expect(after.node_version).toBeGreaterThan(before.node_version);
    expect(after.file_state).toBe('pending_write');
    await expect(page.locator('#g-detail .simple')).toContainText('refunds too');
    await expect(page.locator('#g-sync')).toContainText('pending sync');
  });

  test('configure an edge: change its end → the edge moves on the server', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await selectEdge(page, ids.uc_pay, 'uses', ids.webapp);
    await expect(page.locator('#g-detail')).toContainText('use case uses part');
    await page.locator('#g-detail .actions [data-act=configure]').click();
    await expect(dialog(page)).toBeVisible();
    await shot(page, 'graph-configure-edge');
    await dialog(page).locator('#kit-cf-2').selectOption(ids.checkout);
    await dialog(page).locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    await expect.poll(async () => { const g = await admin.graph(PROJ); return [hasEdge(g, ids.uc_pay, 'uses', ids.checkout), hasEdge(g, ids.uc_pay, 'uses', ids.webapp)]; }).toEqual([true, false]);
  });

  test('create a node attached to a context → drawn, and on the server with its planned path', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.storefront });
    await page.keyboard.press('n');
    await expect(dialog(page).locator('#nn-name')).toBeVisible();
    // a context's default child is a container, linked by codenodes
    await expect(dialog(page).locator('input[name=nt][value=container]')).toBeChecked();
    await expect(dialog(page).locator('#nn-parent')).toHaveValue(ids.storefront);
    await dialog(page).locator('#nn-name').fill('Search service');
    await dialog(page).locator('#nn-desc').fill('Full-text product search.');
    await shot(page, 'graph-new-node');
    const resp = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/graph/nodes'));
    await dialog(page).locator('.kit-primary').click();
    const created = (await (await resp).json()).node;
    expect(created.path).toBe('{topdir}/src/storefront/search_service/search_service.code.iter.md');
    const g = await admin.graph(PROJ);
    const n = g.nodes.find((x) => x.name === 'Search service');
    expect(n).toMatchObject({ nodetype: 'code', level: 'container', desc: 'Full-text product search.', file_state: 'pending_write' });
    expect(hasEdge(g, ids.storefront, 'codenodes', n.id)).toBe(true);
    await expect.poll(() => page.evaluate((id) => IterGraph.api.cy.getElementById(id).nonempty(), n.id)).toBe(true);
    await expect(page.locator('#g-detail h2')).toHaveText('Search service');
  });

  test('add an edge by drawing it (C, then click the target)', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.http });
    await page.keyboard.press('c');
    await expect(page.locator('#g-banner')).toContainText('Connecting from');
    await shot(page, 'graph-draw-edge');
    await still(page);
    const t = await posOf(page, 'graph', ids.postgres);
    await page.mouse.click(t.x, t.y);
    if (await dialog(page).count()) await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => hasEdge(await admin.graph(PROJ), ids.http, 'connects', ids.postgres)).toBe(true);
    await expect.poll(() => page.evaluate((id) => IterGraph.api.cy.getElementById(id).nonempty(), `e:${ids.http}|connects|${ids.postgres}`)).toBe(true);
  });

  test('remove an edge: asks why, then it is gone', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await selectEdge(page, ids.shopper, 'touches', ids.webapp);
    await page.keyboard.press('Delete');
    await expect(dialog(page).locator('textarea')).toBeVisible();
    // the reason is required
    await dialog(page).locator('.kit-danger').click();
    await expect(dialog(page)).toBeVisible();
    await shot(page, 'graph-remove-edge');
    const req = page.waitForRequest((r) => r.method() === 'DELETE' && r.url().endsWith('/graph/edges'));
    await dialog(page).locator('textarea').fill('the shopper only touches the checkout form');
    await dialog(page).locator('.kit-danger').click();
    expect(JSON.parse((await req).postData())).toMatchObject({ from: ids.shopper, to: ids.webapp, kind: 'touches', reason: 'the shopper only touches the checkout form' });
    await expect.poll(async () => hasEdge(await admin.graph(PROJ), ids.shopper, 'touches', ids.webapp)).toBe(false);
  });

  test('drag an edge end onto another node → the edge moves on the server', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await selectEdge(page, ids.http, 'connects', ids.webapp);
    await expect.poll(() => page.evaluate(() => IterGraph.api.cy.getElementById('__kit_ep_t').nonempty())).toBe(true);
    await shot(page, 'graph-edge-handles');
    const req = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith('/graph/edges/move'));
    await dragHandle(page, 'graph', 'target', ids.settle);
    expect(JSON.parse((await req).postData())).toEqual({ from: ids.http, to: ids.webapp, kind: 'connects', new_to: ids.settle });
    await expect.poll(async () => { const g = await admin.graph(PROJ); return [hasEdge(g, ids.http, 'connects', ids.settle), hasEdge(g, ids.http, 'connects', ids.webapp)]; }).toEqual([true, false]);
    // the file that owns the edge (the connection's connects.to) changed
    expect((await admin.get(`${P}/graph/nodes/${ids.http}`)).file_state).toBe('pending_write');
  });

  test('copy an edge and paste it onto another node', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await selectEdge(page, ids.gateway, 'tests', ids.t_gateway);
    await page.keyboard.press('ControlOrMeta+c');
    await expect(page.locator('#g-editbar [data-ed=paste]')).toBeDisabled(); // nothing selected to paste onto yet
    await select(page, { kind: 'node', id: ids.ledger });
    await expect(page.locator('#g-editbar [data-ed=paste]')).toBeEnabled();
    await page.keyboard.press('ControlOrMeta+v');
    if (await dialog(page).count()) await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => hasEdge(await admin.graph(PROJ), ids.ledger, 'tests', ids.t_gateway)).toBe(true);
    // the original is still there
    expect(hasEdge(await admin.graph(PROJ), ids.gateway, 'tests', ids.t_gateway)).toBe(true);
  });

  test('delete a node: asks why, then it is gone from the graph (pending delete)', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.checkout });
    await page.locator('#g-detail .actions [data-act=delete]').click();
    await expect(dialog(page).locator('textarea')).toBeVisible();
    await shot(page, 'graph-delete-node');
    const req = page.waitForRequest((r) => r.method() === 'DELETE' && r.url().endsWith(`/graph/nodes/${ids.checkout}`));
    await dialog(page).locator('textarea').fill('merged into the web app');
    await dialog(page).locator('.kit-danger').click();
    expect(JSON.parse((await req).postData())).toEqual({ reason: 'merged into the web app' });
    await expect.poll(async () => (await admin.graph(PROJ)).nodes.some((n) => n.id === ids.checkout)).toBe(false);
    const g = await admin.graph(PROJ);
    expect(hasEdge(g, ids.webapp, 'codenodes', ids.checkout)).toBe(false);
    await expect.poll(() => page.evaluate((id) => IterGraph.api.cy.getElementById(id).empty(), ids.checkout)).toBe(true);
    const pend = await new Api(readState().base, readState().engineToken).get(`${P}/files/pending`);
    expect(pend.pending.some((x) => x.id === ids.checkout && x.op === 'delete')).toBe(true);
  });
});
