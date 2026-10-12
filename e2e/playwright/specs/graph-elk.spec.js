// Project graph, the ELK layouts (2026-10-11): Layered → / Layered ↓ (boxes inside boxes in layers,
// lines routed at right angles around the boxes) and Boxes (packed boxes, faint lines until hover).
const { test, expect, openTab, posOf, still, readState, Api, shot } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'elk';
let ids;

const pick = async (page, layout) => {
  await page.click(`#g-layouts button[data-layout="${layout}"]`);
  // the ELK layouts run asynchronously and hide the canvas until they are done
  await page.waitForFunction(() => { const c = IterGraph.api.cy.container(); return c && c.style.opacity === '' && IterGraph.api.layout; });
  await page.waitForTimeout(400);
  await still(page);
};
// sibling boxes, and leaves outside a box, that overlap (drawn with their names)
const overlaps = (page) => page.evaluate(() => {
  const cy = IterGraph.api.cy; let n = 0;
  const ov = (a, b) => a.x1 < b.x2 && b.x1 < a.x2 && a.y1 < b.y2 && b.y1 < a.y2;
  const els = cy.nodes().filter((x) => !x.id().startsWith('__')).map((x) => ({ x, b: x.boundingBox({ includeLabels: true, includeOverlays: false }) }));
  for (let i = 0; i < els.length; i++) for (let j = i + 1; j < els.length; j++) {
    const A = els[i].x; const B = els[j].x;
    if (A.ancestors().contains(B) || B.ancestors().contains(A)) continue;
    if (ov(els[i].b, els[j].b)) n++;
  }
  return n;
});

test.describe('project graph: ELK layouts', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async () => {
    const st = readState();
    ids = (await seedProject(new Api(st.base, st.adminToken), new Api(st.base, st.engineToken), PROJ, { conflict: false, pending: false, items: false })).ids;
  });

  for (const layout of ['layered', 'layered-down']) {
    test(`${layout}: boxes inside boxes, nothing overlaps, lines follow their routes`, async ({ page }) => {
      await openTab(page, 'graph', PROJ);
      await pick(page, layout);
      expect(await page.evaluate(() => IterGraph.api.layout)).toBe(layout);
      const st = await page.evaluate(() => {
        const cy = IterGraph.api.cy;
        const routed = cy.edges('.routed');
        // each routed line passes through the points ELK chose (Cytoscape draws what was computed)
        const off = routed.map((e) => { const want = e.scratch('_route').pts; const got = e.segmentPoints() || []; return Math.max(...want.map((p, i) => (got[i] ? Math.hypot(p.x - got[i].x, p.y - got[i].y) : 999))); });
        const wi = off.indexOf(Math.max(0, ...off)); const we = routed[wi];
        const why = we ? { id: we.id(), srcBox: we.source().isParent(), tgtBox: we.target().isParent(), S: we.source().position(), T: we.target().position(), at: we.scratch('_route').at } : null;
        return { boxes: cy.nodes(':parent').length, routed: routed.length, worst: Math.max(0, ...off), why };
      });
      expect(st.boxes).toBeGreaterThan(0);
      expect(st.routed).toBeGreaterThan(0);
      expect(st.worst, JSON.stringify(st.why)).toBeLessThan(1);
      expect(await overlaps(page)).toBe(0);
      await shot(page, `graph-${layout}`);
      // a moved node's lines go back to the chosen line style
      const a = await posOf(page, 'graph', ids.gateway);
      await page.mouse.move(a.x, a.y); await page.mouse.down();
      await page.mouse.move(a.x + 60, a.y + 40, { steps: 6 }); await page.mouse.up();
      await still(page);
      expect(await page.evaluate((id) => IterGraph.api.cy.getElementById(id).connectedEdges('.routed').length, ids.gateway)).toBe(0);
    });
  }

  test('boxes: packed boxes, faint lines that light up on hover', async ({ page }) => {
    await openTab(page, 'graph', PROJ);
    await pick(page, 'boxes');
    expect(await overlaps(page)).toBe(0);
    const quiet = () => page.evaluate((id) => {
      const cy = IterGraph.api.cy; const es = cy.getElementById(id).connectedEdges();
      return { all: cy.edges('.quiet').length === cy.edges().length, lit: es.filter((e) => e.hasClass('hl')).length, n: es.length };
    }, ids.gateway);
    let q = await quiet();
    expect(q.all).toBe(true);
    expect(q.lit).toBe(0);
    await shot(page, 'graph-boxes');
    const a = await posOf(page, 'graph', ids.gateway);
    await page.mouse.move(a.x, a.y);
    await expect.poll(async () => (await quiet()).lit).toBeGreaterThan(0);
    q = await quiet();
    expect(q.lit).toBe(q.n);
  });
});
