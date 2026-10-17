// Designer → build: the Intro wizard creates a designed project (no engine, no
// repository), the Project graph shows the "Designed — not built" banner, Build
// picks an engine + folder and POSTs /build; the banner turns to "Building".
// An engine the server does not know is refused in the dialog, before any request.
const { test, expect, openTab, dialog, shot } = require('../lib/fixtures');
const { sha } = require('../lib/seed');

const PROJ = 'garden';

test.describe('designer', () => {
  test('wizard → designed project → banner → Build dialog → POST build → engine builds it', async ({ page, admin, engine }) => {
    await openTab(page, 'intro', 'shop');
    await page.click('button[data-page=wizard]');
    await page.fill('#intro-f-name', PROJ);
    await page.fill('#intro-f-desc', 'A garden planner: beds, plants and a watering schedule.');
    await page.click('[data-wnext]');
    await shot(page, 'designer-wizard-review');
    const put = page.waitForRequest((r) => r.method() === 'PUT' && r.url().endsWith(`/api/projects/${PROJ}`));
    await page.click('[data-wcreate]');
    expect(JSON.parse((await put).postData())).toMatchObject({ name: PROJ, state: 'Stopped' });
    await page.waitForSelector('.intro-route');
    await shot(page, 'designer-wizard-done');

    // the server made the project node and the default global reqs, all designed
    const g0 = await admin.graph(PROJ);
    expect(g0.nodes.filter((n) => n.nodetype !== 'req').map((n) => n.nodetype).sort()).toEqual(['bizreq', 'philosophy', 'project', 'techreq']);
    expect(g0.nodes.every((n) => n.file_state === 'designed')).toBe(true);
    expect(g0.nodes.find((n) => n.nodetype === 'project').path).toBe(`{topdir}/global/${PROJ}.project.iter.md`);

    await page.click('[data-wopen]');
    await page.waitForFunction(() => window.IterGraph && IterGraph.api && IterGraph.api.model && IterGraph.api.model.G.nodes.length > 0);
    await expect(page.locator('#g-designbar')).toBeVisible();
    await expect(page.locator('#g-designbar')).toContainText('Designed — not built yet');
    await expect(page.locator('#g-sync')).toContainText('4 designed');
    await page.waitForTimeout(400);
    await shot(page, 'designer-graph');

    await page.click('#g-designbar [data-build]');
    await expect(dialog(page).locator('#b-top')).toHaveValue(`~/dev/${PROJ}`);
    await expect(dialog(page).locator('#b-eng option[value=mbp]')).toHaveCount(1);
    await dialog(page).locator('#b-eng').selectOption('mbp');
    await dialog(page).locator('#b-top').fill(`/tmp/iter5_pw/${PROJ}`);
    await dialog(page).locator('#b-note').fill('Start with the beds and plants model.');
    await shot(page, 'designer-build-dialog');
    const reqP = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith(`/api/projects/${PROJ}/build`));
    const respP = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith(`/api/projects/${PROJ}/build`));
    await dialog(page).locator('.kit-primary').click();
    expect(JSON.parse((await reqP).postData())).toEqual({ engine: 'mbp', topdir: `/tmp/iter5_pw/${PROJ}`, queue_plan: true, plan_note: 'Start with the beds and plants model.' });
    const resp = await respP;
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.build).toMatchObject({ state: 'requested', engine: 'mbp', topdir: `/tmp/iter5_pw/${PROJ}`, queue_plan: true });
    expect(body.flipped).toBe(4);
    expect(body.serves_edge).toBeTruthy();
    await expect(dialog(page)).toHaveCount(0);
    await expect(page.locator('#g-designbar')).toContainText('Building on mbp');
    await expect(page.locator('#g-sync')).toContainText('pending sync');
    await shot(page, 'designer-building');

    // the server side: build requested, nodes pending write, a serves edge, a plan item at priority 5
    const b = await admin.get(`/api/projects/${PROJ}/build`);
    expect(b.build.state).toBe('requested');
    expect(b.served).toBe(true);
    const g1 = await admin.graph(PROJ);
    expect(g1.nodes.every((n) => n.file_state === 'pending_write')).toBe(true);
    const sg = await admin.get('/api/settings/graph');
    expect(sg.edges.some((e) => e.type === 'serves' && e.from === 'iter_engine:mbp' && e.to === `project:${PROJ}` && e.active && e.settings.topdir === `/tmp/iter5_pw/${PROJ}`)).toBe(true);

    // the engine takes the build: writes + acks every file, reports done → the page notices on its own
    const pend = (await engine.get(`/api/projects/${PROJ}/files/pending`)).pending;
    expect(pend.length).toBe(4);
    await engine.post(`/api/projects/${PROJ}/files/ack`, { engine: 'mbp', acks: pend.map((x) => ({ id: x.id, node_version: x.node_version, path: x.path, hash: sha(x.text || ''), commit: 'beefcafe' })) });
    await engine.post(`/api/projects/${PROJ}/build/done`, { engine: 'mbp', commit: 'beefcafe' });
    await expect(page.locator('#g-designbar')).toBeHidden({ timeout: 20000 });
    await expect(page.locator('#g-buildstate')).toContainText('built on mbp');
    await expect(page.locator('#g-sync')).toContainText('synced', { timeout: 20000 });
    // queue_plan: the plan item is queued once the repository exists
    const items = await admin.get(`/api/projects/${PROJ}/workitems`);
    expect(items.some((i) => i.agent === 'plan' && i.priority === 5 && i.state === 'queued')).toBe(true);
    await shot(page, 'designer-built');
  });

  test('Build refuses an engine that is not registered, without a request or a console error', async ({ page, admin }) => {
    await admin.put('/api/projects/sketch', { state: 'Stopped', desc: 'A sketch.' });
    // no engine registered as far as this page knows: the dialog falls back to a free-text engine name
    await page.route(/\/api\/engines$/, (r) => r.fulfill({ json: [] }));
    await page.route(/\/api\/settings\/graph$/, async (r) => { const res = await r.fetch(); const j = await res.json(); j.nodes = j.nodes.filter((n) => n.type !== 'iter_engine'); await r.fulfill({ json: j }); });
    let posted = false;
    page.on('request', (r) => { if (r.method() === 'POST' && r.url().endsWith('/build')) posted = true; });
    await openTab(page, 'graph', 'sketch');
    await expect(page.locator('#g-designbar')).toContainText('Designed — not built yet');
    await page.click('#g-designbar [data-build]');
    await expect(dialog(page).locator('input#b-eng')).toBeVisible();
    await dialog(page).locator('#b-eng').fill('ghost');
    await dialog(page).locator('.kit-primary').click();
    await expect(dialog(page)).toContainText('There is no engine called "ghost"');
    await shot(page, 'designer-build-unknown-engine');
    expect(posted).toBe(false);
    await dialog(page).locator('button', { hasText: 'Cancel' }).click();
    expect((await admin.get('/api/projects/sketch/build')).build).toBeNull();
  });
});
