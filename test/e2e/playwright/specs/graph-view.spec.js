// Project graph, read side: every node type is drawn, the type filter chips and
// presets hide/show and persist in the URL hash, and the detail pane shows the
// node, its links, test results + test log history, sync conflicts and build state.
const { test, expect, openTab, posOf, shot } = require('../lib/fixtures');

const drawn = (page) => page.evaluate(() => IterGraph.api.cy.nodes().filter((n) => !n.id().startsWith('__') && !n.hasClass('kit-ep')).map((n) => n.id()));
const drawnLevels = (page) => page.evaluate(() => [...new Set(IterGraph.api.cy.nodes().map((n) => n.data('level')).filter(Boolean))].sort());

test.describe('project graph: view', () => {
  test('draws every node of every type, and the links', async ({ page, admin }) => {
    const g = await admin.graph('shop');
    await openTab(page, 'graph', 'shop');
    const ids = await drawn(page);
    // requirement sections (req nodes) are hidden by default: every file node is drawn
    const files = g.nodes.filter((n) => n.nodetype !== 'req');
    for (const n of files) expect(ids, `${n.nodetype} ${n.name}`).toContain(n.id);
    expect(ids.filter((id) => g.nodes.some((n) => n.id === id && n.nodetype === 'req'))).toEqual([]);
    const levels = await drawnLevels(page);
    for (const lv of ['project', 'context', 'container', 'component', 'connection', 'test', 'bizreq', 'techreq', 'philosophy', 'usecase', 'actor']) expect(levels).toContain(lv);
    // every non-ownership link is an edge on the canvas (ownership is drawn as boxes in Cluster)
    const edgeIds = await page.evaluate(() => IterGraph.api.cy.edges().map((e) => e.id()));
    for (const e of g.edges.filter((x) => x.kind !== 'codenodes' && x.kind !== 'contains')) expect(edgeIds).toContain(`e:${e.from}|${e.kind}|${e.to}`);
    await expect(page.locator('#g-summary')).toContainText(`${files.length} nodes`);
    // connection nodes are diamonds
    const shape = await page.evaluate((id) => IterGraph.api.cy.getElementById(id).style('shape'), g.nodes.find((n) => n.level === 'connection').id);
    expect(shape).toBe('diamond');
    await shot(page, 'graph-all');
    // the legend opens and closes, and stays as left (per browser)
    await page.click('#g-legendToggle');
    await expect(page.locator('#g-legend .legendbody')).toBeVisible();
    await expect(page.locator('#g-legend')).toContainText('Connection (a type of link');
    await shot(page, 'graph-legend');
    await page.click('#g-legendToggle');
    await expect(page.locator('#g-legend .legendbody')).toBeHidden();
  });

  test('type filter chips hide and show, presets, hash persistence', async ({ page, admin }) => {
    const g = await admin.graph('shop');
    const tests = g.nodes.filter((n) => n.nodetype === 'test').map((n) => n.id);
    await openTab(page, 'graph', 'shop');
    await page.click('#g-chips button[data-type=test]');
    await expect(page.locator('#g-chips button[data-type=test]')).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(async () => (await drawn(page)).filter((id) => tests.includes(id)).length).toBe(0);
    expect(new URLSearchParams((await page.evaluate(() => location.hash)).slice(1)).get('hide')).toBe('req,test'); // req: requirement sections, hidden by default
    // a reload keeps it (hash)
    await page.reload();
    await page.waitForFunction(() => window.IterGraph && IterGraph.api && IterGraph.api.cy && IterGraph.api.cy.nodes().length > 0);
    await expect(page.locator('#g-chips button[data-type=test]')).toHaveAttribute('aria-pressed', 'false');
    expect((await drawn(page)).filter((id) => tests.includes(id))).toEqual([]);
    // shown again
    await page.click('#g-chips button[data-type=test]');
    await expect.poll(async () => (await drawn(page)).filter((id) => tests.includes(id)).length).toBe(tests.length);
    // Network map preset: code + connections only, flow layout
    await page.click('#g-presets button[data-preset=network]');
    await expect(page.locator('#g-presets button[data-preset=network]')).toHaveClass(/on/);
    await expect.poll(() => drawnLevels(page)).toEqual(['component', 'connection', 'container', 'context']);
    const hp = new URLSearchParams((await page.evaluate(() => location.hash)).slice(1));
    expect(hp.get('layout')).toBe('flow');
    expect(hp.get('hide').split(',').sort()).toEqual(['actor', 'bizreq', 'philosophy', 'project', 'req', 'techreq', 'test', 'usecase']);
    await shot(page, 'graph-network-map');
    // the hash alone reproduces the view (a copied link)
    const url = page.url();
    const page2 = await page.context().newPage();
    await page2.goto(url);
    await page2.waitForFunction(() => window.IterGraph && IterGraph.api && IterGraph.api.cy && IterGraph.api.cy.nodes().length > 0);
    await expect.poll(() => drawnLevels(page2)).toEqual(['component', 'connection', 'container', 'context']);
    await page2.close();
    // alt-click: only that type
    await page.click('#g-presets button[data-preset=all]');
    await page.click('#g-chips button[data-type=actor]', { modifiers: ['Alt'] });
    await expect.poll(() => drawnLevels(page)).toEqual(['actor']);
    await page.click('#g-presets button[data-preset=all]');
    await expect.poll(async () => (await drawn(page)).length).toBe(g.nodes.filter((n) => n.nodetype !== 'req').length);
  });

  test('detail pane: a code node, a connection, a test with its log, a conflict, the build', async ({ page, admin, state }) => {
    const ids = state.shop.ids;
    await openTab(page, 'graph', 'shop');
    // click a node on the canvas
    const p = await posOf(page, 'graph', ids.gateway);
    await page.mouse.click(p.x, p.y);
    const box = page.locator('#g-detail');
    await expect(box).toBeVisible();
    await expect(box.locator('h2')).toHaveText('API gateway');
    await expect(box).toContainText('Kong in front of every backend service');
    await expect(box).toContainText('{topdir}/src/storefront/api_gateway/api_gateway.code.iter.md');
    await expect(box).toContainText('Gateway smoke');
    // its requirements files: the techreq file with its two sections, no bizreq file yet
    await expect(box.locator('.g-rfile.ft-techreq .g-count')).toHaveText('2');
    await expect(box.locator('.g-rfile.ft-bizreq')).toContainText('no file yet');
    await expect(box.locator('.g-fs')).toHaveText('synced');
    await expect(box.locator('.actions [data-act=configure]')).toBeVisible();
    await shot(page, 'graph-detail-code');

    // a connection: who supplies it, what it reaches
    await page.evaluate((id) => { IterGraph.api.select({ kind: 'node', id }); }, ids.http);
    await expect(box.locator('.g-conn')).toContainText('API gateway');
    await expect(box.locator('.g-conn')).toContainText('Ledger service');
    await expect(box.locator('.g-conn')).toContainText('Web app');

    // a test node: last result and the test log history (GET testlogs?node=)
    const logs = (await admin.get(`/api/projects/shop/testlogs?node=${ids.t_ledger}&limit=20`)).logs;
    expect(logs.length).toBe(2);
    await page.evaluate((id) => { IterGraph.api.select({ kind: 'node', id }); }, ids.t_ledger);
    await expect(box.locator('.g-test')).toContainText('failing');
    await expect(box).toContainText('refund balances');
    const hist = box.locator('.g-testlogs');
    await expect(hist).toBeVisible();
    await expect(hist.locator('.g-tlog')).toHaveCount(2);
    await expect(hist.locator('.g-tlog').first()).toContainText('fail');
    await expect(hist.locator('.g-tlog').nth(1)).toContainText('pass');
    await shot(page, 'graph-detail-test');

    // a conflict (GET graph/conflicts): a toolbar badge and the node's own list
    const conflicts = (await admin.get('/api/projects/shop/graph/conflicts')).conflicts;
    expect(conflicts.length).toBeGreaterThan(0);
    await expect(page.locator('#g-conflicts')).toContainText(String(conflicts.length));
    await page.evaluate((id) => { IterGraph.api.select({ kind: 'node', id }); }, ids.postgres);
    await expect(box.locator('.g-conflicts')).toContainText("kept the graph's version");
    await expect(box.locator('.g-conflicts')).toContainText(conflicts[0].why);
    await shot(page, 'graph-detail-conflict');
    await page.click('#g-conflicts');
    await expect(page.locator('dialog.kit-dlg[open]')).toContainText('Postgres');
    await shot(page, 'graph-conflicts-list');
    await page.keyboard.press('Escape');

    // the build state: a badge in the toolbar and a Repository block on the project node
    const b = await admin.get('/api/projects/shop/build');
    expect(b.build.state).toBe('done');
    await expect(page.locator('#g-buildstate')).toContainText('mbp');
    await page.evaluate((id) => { IterGraph.api.select({ kind: 'node', id }); }, ids.project);
    await expect(box.locator('.g-buildinfo')).toContainText('/tmp/iter5_pw/shop');
    await expect(box.locator('.g-buildinfo')).toContainText('c0ffee0');
    await shot(page, 'graph-detail-project');
  });
});
