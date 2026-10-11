// Project graph edges (2026-10-11): a calls / pulls / pushes edge carries a protocol and is drawn
// "calls (gRPC)"; two edges between the same nodes never lie on top of each other; lines can be
// drawn straight, curved or elbow.
const { test, expect, openTab, posOf, still, dialog, shot, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'wires';
const P = `/api/projects/${PROJ}`;
let ids;

const edgeOf = (g, from, kind, to) => g.edges.find((e) => e.from === from && e.kind === kind && e.to === to);
const select = async (page, sel) => { await page.evaluate((s) => { IterGraph.api.select(s); }, sel); await still(page); };
const drawn = (page, from, kind, to) => page.evaluate((id) => {
  const e = IterGraph.api.cy.getElementById(id); if (e.empty()) return null;
  const m = e.midpoint(); const pts = [e.sourceEndpoint(), ...(e.style('curve-style') === 'taxi' ? e.segmentPoints() || [] : []), e.targetEndpoint()];
  return { label: e.data('label'), curve: e.style('curve-style'), x: m.x, y: m.y, pts, classes: e.classes() };
}, `e:${from}|${kind}|${to}`);
const apart = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
/** the widest gap between two lines' corresponding points (ends and bends) */
const spread = (a, b) => Math.max(apart(a, b), ...a.pts.map((p, i) => (b.pts[i] ? Math.hypot(p.x - b.pts[i].x, p.y - b.pts[i].y) : 0)));
/** no leg (segment between consecutive points) of one line lies on a leg of the other; crossing at a point is fine */
const same = (p, q) => Math.hypot(p.x - q.x, p.y - q.y) <= 3;
const noSharedLeg = (a, b) => a.pts.slice(1).every((p, i) => !b.pts.slice(1).some((q, j) => same(a.pts[i], b.pts[j]) && same(p, q)));

test.describe('project graph: edge protocols, parallel edges and line shapes', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async () => {
    const st = readState();
    ids = (await seedProject(new Api(st.base, st.adminToken), new Api(st.base, st.engineToken), PROJ, { conflict: false, pending: false, items: false })).ids;
  });

  test('drawing a calls edge asks over what, and the line says "calls (gRPC)"', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.webapp });
    await page.keyboard.press('c');
    await expect(page.locator('#g-banner')).toContainText('Connecting from');
    await page.evaluate((id) => { IterGraph.api.cy.getElementById(id).emit('tap'); }, ids.gateway);
    await expect(dialog(page)).toContainText('What does this edge mean?');
    await dialog(page).locator('.kit-primary').click(); // "Web app calls API gateway" is first
    await expect(dialog(page)).toContainText('over what?');
    await dialog(page).locator('#pp-sel').selectOption('gRPC');
    await shot(page, 'graph-edge-protocol');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => (edgeOf(await admin.graph(PROJ), ids.webapp, 'calls', ids.gateway) || {}).protocol).toBe('gRPC');
    await expect.poll(async () => (await drawn(page, ids.webapp, 'calls', ids.gateway) || {}).label).toBe('calls (gRPC)');
  });

  test('two edges between the same nodes are drawn apart, in every line shape', async ({ page, admin }) => {
    await admin.post(`${P}/graph/edges`, { from: ids.webapp, kind: 'pushes', to: ids.gateway, protocol: 'UDP' });
    await openTab(page, 'graph', PROJ);
    for (const mode of ['straight', 'curved', 'elbow']) {
      await page.locator('#g-lines').selectOption(mode);
      await still(page);
      const a = await drawn(page, ids.webapp, 'calls', ids.gateway);
      const b = await drawn(page, ids.webapp, 'pushes', ids.gateway);
      expect(a && b, mode).toBeTruthy();
      expect(b.label).toBe('pushes (UDP)');
      expect(a.classes).toContain('lm-' + mode);
      expect(a.curve).toBe(mode === 'elbow' ? 'taxi' : 'unbundled-bezier');
      expect(spread(a, b), `${mode}: the two lines lie apart`).toBeGreaterThan(10);
      if (mode === 'elbow') expect(noSharedLeg(a, b), 'elbow: the two lines share no leg').toBe(true);
      if (mode === 'elbow') await shot(page, 'graph-edges-elbow');
    }
    // the choice is remembered in this browser
    await page.reload(); await openTab(page, 'graph', PROJ);
    await expect(page.locator('#g-lines')).toHaveValue('elbow');
    await page.locator('#g-lines').selectOption('straight');
  });

  test('nested nodes stay visible: fully while connecting, faintly when faded by a selection', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    const op = (id) => page.evaluate((i) => IterGraph.api.cy.getElementById(i).effectiveOpacity(), id);
    // a plain selection fades what is unrelated, but a node inside a faded box is still there
    await select(page, { kind: 'node', id: ids.checkout });
    expect(await op(ids.settle)).toBeGreaterThanOrEqual(0.15);
    // connecting: nothing is faded, nested source and nested target alike
    await page.keyboard.press('c');
    await expect(page.locator('#g-banner')).toContainText('Connecting from');
    expect(await op(ids.checkout)).toBeGreaterThan(0.9);
    expect(await op(ids.settle)).toBeGreaterThan(0.9);
    await shot(page, 'graph-connect-nested');
    await page.keyboard.press('Escape');
  });

  test('the edge menu changes the protocol', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await page.evaluate((id) => IterGraph.api.focusEdgeId(id), `e:${ids.webapp}|calls|${ids.gateway}`);
    await still(page);
    const p = await page.evaluate((id) => { const e = IterGraph.api.cy.getElementById(id); const box = IterGraph.api.cy.container().getBoundingClientRect(); const m = e.renderedMidpoint(); return { x: box.left + m.x, y: box.top + m.y }; }, `e:${ids.webapp}|calls|${ids.gateway}`);
    await page.mouse.click(p.x, p.y, { button: 'right' });
    await expect(page.locator('.kit-menu')).toBeVisible();
    await page.locator('.kit-menu button', { hasText: 'Protocol' }).click();
    await dialog(page).locator('#pp-sel').selectOption('__other');
    await dialog(page).locator('#pp-other').fill('NATS');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => (edgeOf(await admin.graph(PROJ), ids.webapp, 'calls', ids.gateway) || {}).protocol).toBe('NATS');
    await expect(page.locator('#g-detail')).toContainText('Protocol: NATS');
  });
});
