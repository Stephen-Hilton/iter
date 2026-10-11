// Project graph: pausing graph → file sync while building (2026-10-11). Edits made while paused stay
// in the graph and are not handed to an engine; resuming releases them all.
const { test, expect, openTab, still, dialog, shot, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'drafts';
const P = `/api/projects/${PROJ}`;
let ids;

test.describe('project graph: pause file sync', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async () => {
    const st = readState();
    ids = (await seedProject(new Api(st.base, st.adminToken), new Api(st.base, st.engineToken), PROJ, { conflict: false, pending: false, items: false, built: true })).ids;
  });

  test('pause, build, resume', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    await expect(page.locator('#g-syncpause')).toHaveText(/Pause sync/);
    await page.locator('#g-syncpause').click();
    await expect(page.locator('#g-sync')).toContainText('sync paused');
    expect((await admin.get(`${P}/files/pause`)).paused).toBe(true);
    // build while paused: the edit is held, and no engine is offered it
    await admin.post(`${P}/graph/nodes`, { nodetype: 'code', level: 'container', name: 'Draft service', attach_to: ids.storefront, attach_kind: 'codenodes' });
    const held = await admin.get(`${P}/files/pending`);
    expect(held.count).toBe(0);
    expect(held.held).toBeGreaterThan(0);
    await page.reload(); await openTab(page, 'graph', PROJ);
    await expect(page.locator('#g-sync')).toContainText('held');
    await expect(page.locator('#g-syncpause')).toHaveText(/Resume sync/);
    // paused: removals ask no "why?" — an edge goes at once, a node after a plain yes/no
    await page.evaluate((id) => IterGraph.api.focusEdgeId(id), `e:${ids.uc_pay}|uses|${ids.webapp}`);
    await still(page);
    await page.keyboard.press('Delete');
    await expect.poll(async () => (await admin.graph(PROJ)).edges.some((e) => e.from === ids.uc_pay && e.kind === 'uses' && e.to === ids.webapp)).toBe(false);
    const draft = (await admin.graph(PROJ)).nodes.find((n) => n.name === 'Draft service');
    await page.evaluate((id) => IterGraph.api.select({ kind: 'node', id }), draft.id);
    await still(page);
    await page.keyboard.press('Delete');
    await expect(dialog(page)).toContainText('Delete Draft service?');
    await expect(dialog(page).locator('textarea')).toHaveCount(0);
    await dialog(page).locator('.kit-danger, .kit-primary').first().click();
    // never written to the repo: it is simply gone
    await expect.poll(async () => (await admin.graph(PROJ)).nodes.some((n) => n.id === draft.id)).toBe(false);
    await shot(page, 'graph-sync-paused');
    // resume: asks first, then everything held is offered again
    await page.locator('#g-syncpause').click();
    await expect(dialog(page)).toContainText('Resume file sync?');
    await dialog(page).locator('.kit-primary').click();
    await expect(page.locator('#g-syncpause')).toHaveText(/Pause sync/);
    expect((await admin.get(`${P}/files/pause`)).paused).toBe(false);
    expect((await admin.get(`${P}/files/pending`)).count).toBeGreaterThan(0);
  });
});
