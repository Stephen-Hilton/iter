// Project graph, Cluster layout (2026-10-11): the "auto-fit nodes" switch, and a pinned context
// that holds its upper-left corner while what is inside it stays free.
const { test, expect, openTab, posOf, still, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'layouts';
let ids;

const select = async (page, sel) => { await page.evaluate((s) => { IterGraph.api.select(s); }, sel); await still(page); };
const leafPositions = (page) => page.evaluate(() => {
  const out = {}; IterGraph.api.cy.nodes().forEach((n) => { if (!n.isParent() && !n.id().startsWith('__')) out[n.id()] = { x: Math.round(n.position('x')), y: Math.round(n.position('y')) }; });
  return out;
});
const corner = (page, id) => page.evaluate((i) => { const bb = IterGraph.api.cy.getElementById(i).boundingBox({ includeLabels: false, includeOverlays: false }); return { x1: Math.round(bb.x1), y1: Math.round(bb.y1) }; }, id);
const autofit = async (page, on) => {
  const box = page.locator('#g-autofit');
  await expect(box).toBeVisible();
  if ((await box.isChecked()) !== on) await box.click();
  await still(page);
};

test.describe('project graph: auto-fit and wrapper pins', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async () => {
    const st = readState();
    ids = (await seedProject(new Api(st.base, st.adminToken), new Api(st.base, st.engineToken), PROJ, { conflict: false, pending: false, items: false })).ids;
  });

  test('auto-fit off: a drag moves only that node, and positions survive a reload', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    expect(await page.evaluate(() => IterGraph.api.layout)).toBe('cluster');
    await autofit(page, false);
    const before = await leafPositions(page);
    const a = await posOf(page, 'graph', ids.gateway);
    await page.mouse.move(a.x, a.y); await page.mouse.down();
    await page.mouse.move(a.x + 140, a.y + 90, { steps: 10 }); await page.mouse.up();
    await page.waitForTimeout(900); // a settle (auto-fit on) would animate for ~450 ms
    const after = await leafPositions(page);
    const moved = Object.keys(before).filter((k) => after[k] && (Math.abs(after[k].x - before[k].x) > 2 || Math.abs(after[k].y - before[k].y) > 2));
    expect(moved).toEqual([ids.gateway]);
    await page.reload();
    await openTab(page, 'graph', PROJ);
    await expect(page.locator('#g-autofit')).not.toBeChecked();
    const reloaded = await leafPositions(page);
    expect(reloaded[ids.gateway]).toEqual(after[ids.gateway]);
    expect(reloaded[ids.ledger]).toEqual(after[ids.ledger]);
  });

  test('a pinned context keeps its upper-left corner through a re-layout; what is inside stays free', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    await autofit(page, true);
    await select(page, { kind: 'node', id: ids.storefront });
    await page.keyboard.press('p');
    await still(page);
    const pinned = await corner(page, ids.storefront);
    const state = await page.evaluate((o) => ({ ctx: IterGraph.api.isPinned(o.s), child: IterGraph.api.isPinned(o.w), grab: IterGraph.api.cy.getElementById(o.w).grabbable() }), { s: ids.storefront, w: ids.webapp });
    expect(state).toEqual({ ctx: true, child: false, grab: true });
    await page.locator('#g-relayout').click();
    await still(page);
    await expect.poll(async () => { const c = await corner(page, ids.storefront); return Math.abs(c.x1 - pinned.x1) <= 2 && Math.abs(c.y1 - pinned.y1) <= 2; }).toBe(true);
    // P again unpins it
    await select(page, { kind: 'node', id: ids.storefront });
    await page.keyboard.press('p');
    expect(await page.evaluate((i) => IterGraph.api.isPinned(i), ids.storefront)).toBe(false);
  });
});
