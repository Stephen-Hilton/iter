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
