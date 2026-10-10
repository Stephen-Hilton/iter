// Requirements (spec §2.8): a bizreq/techreq file holds many requirements, one "## " section each, and
// iter_data derives a `req` node per section hanging off its file by a `contains` edge. In the webui:
// the file's detail pane shows them as a table (edit, status, add, delete, move to another file), a code
// node shows its two files (Add creates the file), req nodes are a node type hidden by default, and
// dragging a contains line's file end onto another requirements file moves the requirement.
const { test, expect, openTab, posOf, dragHandle, still, dialog, shot, readState, Api } = require('../lib/fixtures');
const { seedProject } = require('../lib/seed');

const PROJ = 'reqsproj';
const P = `/api/projects/${PROJ}`;
let ids;

/** A file's requirements, in order, as the server has them. */
async function reqsOf(admin, fileId) {
  const g = await admin.graph(PROJ);
  return g.nodes.filter((n) => n.nodetype === 'req' && n.file === fileId).sort((a, b) => a.order - b.order);
}
const fileOfReq = async (admin, id) => ((await admin.graph(PROJ)).nodes.find((n) => n.id === id) || {}).file;
const select = async (page, sel) => { await page.evaluate((s) => { IterGraph.api.select(s); }, sel); await still(page); };
const row = (page, id) => page.locator(`#g-detail tr.g-rrow[data-req="${id}"]`);
const acts = (page, id) => page.locator(`#g-detail [data-req-acts="${id}"]`);
/** Open a requirement's row in the table on screen and press one of its buttons. */
async function rowAction(page, id, act) {
  if (!(await acts(page, id).isVisible())) await row(page, id).locator('td.t').click();
  await acts(page, id).locator(`[data-ract=${act}]`).click();
}
/** Select a contains line and zoom in on it, so its two end handles sit well apart. */
async function pickContains(page, file, id, dropOn) {
  await page.evaluate(([f, r, t]) => {
    const cy = IterGraph.api.cy; const e = cy.getElementById(`e:${f}|contains|${r}`);
    IterGraph.api.select({ kind: 'edge', id: e.id() }, { noPan: true });
    cy.fit(e.union(e.connectedNodes()).union(cy.getElementById(t)), 70);
    if (cy.zoom() > 2) { cy.zoom(2); cy.center(e); }
  }, [file, id, dropOn]);
  await still(page);
  await expect.poll(() => page.evaluate(() => IterGraph.api.cy.getElementById('__kit_ep_s').nonempty())).toBe(true);
}
const showReqNodes = async (page) => {
  await page.click('#g-chips button[data-type=req]');
  await expect(page.locator('#g-chips button[data-type=req]')).toHaveAttribute('aria-pressed', 'true');
  await still(page);
};

test.describe('requirements', () => {
  test.beforeAll(async () => {
    const st = readState();
    const seeded = await seedProject(new Api(st.base, st.adminToken), new Api(st.base, st.engineToken), PROJ, { conflict: false, pending: false, items: false });
    ids = seeded.ids;
  });

  test('a requirements file shows its requirements as a table', async ({ page, admin }) => {
    const want = await reqsOf(admin, ids.bizreq);
    expect(want.length).toBeGreaterThanOrEqual(3);
    await openTab(page, 'graph', PROJ);
    // sections are hidden by default: no req node on the canvas
    expect(await page.evaluate(() => IterGraph.api.cy.nodes('.req').length)).toBe(0);
    await select(page, { kind: 'node', id: ids.bizreq });
    const box = page.locator('#g-detail');
    await expect(box.locator('.g-reqs-head .g-count')).toHaveText(String(want.length));
    await expect(box.locator('tr.g-rrow')).toHaveCount(want.length);
    await expect(box.locator('tr.g-rrow td.k')).toHaveText(want.map((r) => r.key || '—'));
    await expect(box.locator('tr.g-rrow td.s')).toHaveText(want.map((r) => r.status));
    // a row opens to its text (markdown) and its buttons
    const id = ids.req['PAY-BIZ-002'];
    await expect(box.locator(`[data-req-more="${id}"]`)).toBeHidden();
    await row(page, id).click();
    await expect(box.locator(`[data-req-more="${id}"]`)).toBeVisible();
    await expect(box.locator(`[data-req-more="${id}"] .kit-md`)).toContainText('five working days');
    await expect(box.locator(`[data-req-more="${id}"] .kit-md h5, [data-req-more="${id}"] .kit-md h4`)).toHaveText('Why');
    await expect(acts(page, id).locator('[data-ract]')).toHaveText(['Edit…', '+ After', 'Move to…', 'Delete…', 'Show in graph']);
    await row(page, ids.req['PAY-BIZ-001']).click();
    await shot(page, 'req-table');
    // a redraw keeps the open rows open
    await page.keyboard.press('r');
    await expect(box.locator(`[data-req-more="${id}"]`)).toBeVisible();
  });

  test('edit a requirement (Configure) → PATCH graph/reqs, the API has it', async ({ page, admin }) => {
    const id = ids.req['PAY-BIZ-002'];
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.bizreq });
    await rowAction(page, id, 'edit');
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page).locator('#kit-cf-0')).toHaveValue('PAY-BIZ-002');
    await shot(page, 'req-edit');
    const req = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().endsWith(`/graph/reqs/${id}`));
    await dialog(page).locator('#kit-cf-1').fill('Refunds land within three days');
    await dialog(page).locator('#kit-cf-2').selectOption('agreed');
    await dialog(page).locator('.kit-primary').click();
    const body = JSON.parse((await req).postData());
    expect(body).toMatchObject({ title: 'Refunds land within three days', status: 'agreed' });
    expect(body.key).toBeUndefined(); // only what changed
    expect(typeof body.expect_version).toBe('number');
    await expect(dialog(page)).toHaveCount(0);
    await expect.poll(async () => { const r = (await reqsOf(admin, ids.bizreq)).find((x) => x.id === id); return r && [r.title, r.status, r.key]; })
      .toEqual(['Refunds land within three days', 'agreed', 'PAY-BIZ-002']);
    await expect(row(page, id).locator('td.t')).toHaveText('Refunds land within three days');
    await expect(row(page, id).locator('td.s')).toHaveText('agreed');
  });

  test('change a status from its pill', async ({ page, admin }) => {
    const id = ids.req['PAY-BIZ-003'];
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.bizreq });
    await row(page, id).locator('.g-rst').click();
    await expect(page.locator('.kit-menu')).toBeVisible();
    await expect(row(page, id)).toHaveAttribute('aria-expanded', 'false'); // the pill does not fold the row
    await page.locator('.kit-menu button', { hasText: 'agreed' }).click();
    await expect.poll(async () => ((await reqsOf(admin, ids.bizreq)).find((x) => x.id === id) || {}).status).toBe('agreed');
    await expect(row(page, id).locator('td.s')).toHaveText('agreed');
  });

  test('add a requirement → POST graph/reqs, a new row', async ({ page, admin }) => {
    const before = await reqsOf(admin, ids.bizreq);
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.bizreq });
    await page.locator('#g-detail .g-reqs-tools [data-radd]').click();
    await expect(dialog(page).locator('#rq-key')).toHaveValue('PAY-BIZ-004'); // the next key in the file's series
    await dialog(page).locator('#rq-title').fill('Chargebacks are answered within a day');
    await dialog(page).locator('#rq-text').fill('Every chargeback gets evidence filed with the card scheme within 24 hours.');
    await shot(page, 'req-add');
    const resp = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/graph/reqs'));
    await dialog(page).locator('.kit-primary').click();
    const sent = JSON.parse((await resp).request().postData());
    expect(sent).toEqual({ file: ids.bizreq, key: 'PAY-BIZ-004', title: 'Chargebacks are answered within a day', text: 'Every chargeback gets evidence filed with the card scheme within 24 hours.', status: 'draft' });
    const made = (await (await resp).json()).req;
    await expect.poll(async () => (await reqsOf(admin, ids.bizreq)).map((r) => r.key)).toEqual(before.map((r) => r.key).concat('PAY-BIZ-004'));
    await expect(page.locator('#g-detail tr.g-rrow')).toHaveCount(before.length + 1);
    await expect(page.locator('#g-detail .g-reqs-head .g-count')).toHaveText(String(before.length + 1));
    await expect(page.locator(`#g-detail [data-req-more="${made.id}"]`)).toBeVisible(); // the new row opens
  });

  test('delete a requirement: asks why, then it is gone', async ({ page, admin }) => {
    const id = ids.req['GW-TECH-002'];
    const before = await reqsOf(admin, ids.techreq);
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.techreq });
    await rowAction(page, id, 'delete');
    await expect(dialog(page).locator('textarea')).toBeVisible();
    await dialog(page).locator('.kit-danger').click(); // the reason is required
    await expect(dialog(page)).toBeVisible();
    const req = page.waitForRequest((r) => r.method() === 'DELETE' && r.url().endsWith(`/graph/reqs/${id}`));
    await dialog(page).locator('textarea').fill('the gateway vendor rate-limits for us');
    await dialog(page).locator('.kit-danger').click();
    expect(JSON.parse((await req).postData())).toEqual({ reason: 'the gateway vendor rate-limits for us' });
    await expect.poll(async () => (await reqsOf(admin, ids.techreq)).map((r) => r.id)).toEqual(before.map((r) => r.id).filter((x) => x !== id));
    await expect(row(page, id)).toHaveCount(0);
    await expect(page.locator('#g-detail tr.g-rrow')).toHaveCount(before.length - 1);
  });

  test('move a requirement to another file (Move to…) → both files change', async ({ page, admin }) => {
    const id = ids.req['PAY-BIZ-001'];
    const ledger = (await admin.graph(PROJ)).nodes.find((n) => n.id === ids.ledger_techreq);
    const biz0 = await reqsOf(admin, ids.bizreq); const led0 = await reqsOf(admin, ids.ledger_techreq);
    await openTab(page, 'graph', PROJ);
    await select(page, { kind: 'node', id: ids.bizreq });
    await rowAction(page, id, 'move');
    await expect(dialog(page)).toContainText('Move PAY-BIZ-001');
    await dialog(page).locator('label.kit-radio', { hasText: ledger.path }).click();
    await shot(page, 'req-move');
    const req = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith(`/graph/reqs/${id}/move`));
    await dialog(page).locator('.kit-primary').click();
    expect(JSON.parse((await req).postData())).toEqual({ to_file: ids.ledger_techreq });
    await expect.poll(async () => [(await reqsOf(admin, ids.bizreq)).map((r) => r.id), (await reqsOf(admin, ids.ledger_techreq)).map((r) => r.id)])
      .toEqual([biz0.map((r) => r.id).filter((x) => x !== id), led0.map((r) => r.id).concat(id)]);
    await expect(row(page, id)).toHaveCount(0);
    await expect(page.locator('#g-detail tr.g-rrow')).toHaveCount(biz0.length - 1);
    // the ledger's file lists it now (same id)
    await select(page, { kind: 'node', id: ids.ledger_techreq });
    await expect(row(page, id)).toHaveCount(1);
  });

  test('req nodes: hidden by default, drawn beside their file when the type is switched on', async ({ page, admin }) => {
    const g = await admin.graph(PROJ);
    const reqs = g.nodes.filter((n) => n.nodetype === 'req');
    await openTab(page, 'graph', PROJ);
    await expect(page.locator('#g-chips button[data-type=req]')).toHaveAttribute('aria-pressed', 'false');
    await showReqNodes(page);
    expect(new URLSearchParams((await page.evaluate(() => location.hash)).slice(1)).get('hide')).toBe('none');
    const drawn = await page.evaluate(() => IterGraph.api.cy.nodes('.req').map((n) => n.id()));
    expect(drawn.sort()).toEqual(reqs.map((r) => r.id).sort());
    // each hangs off its file by a contains line, in a column just right of it
    const geo = await page.evaluate((list) => list.map((r) => {
      const cy = IterGraph.api.cy; const n = cy.getElementById(r.id); const f = cy.getElementById(r.file);
      return { edge: cy.getElementById(`e:${r.file}|contains|${r.id}`).nonempty(), right: n.position('x') > f.position('x'), near: Math.abs(n.position('y') - f.position('y')) < 120 };
    }), reqs.map((r) => ({ id: r.id, file: r.file })));
    expect(geo.every((x) => x.edge && x.right && x.near)).toBe(true);
    // the Requirements preset keeps the sections toggle as it is, and offers it
    await page.click('#g-presets button[data-preset=reqs]');
    await expect(page.locator('#g-presets button[data-preset=reqs]')).toHaveClass(/on/);
    await expect(page.locator('#g-reqsec')).toBeChecked();
    await still(page);
    // a picture of the requirements files and their sections
    await page.evaluate(() => { const cy = IterGraph.api.cy; const files = cy.nodes('.bizreq, .techreq'); cy.fit(files.union(files.neighborhood()), 50); });
    await still(page);
    await shot(page, 'req-nodes');
    // a req node's own pane: key, title, status, the text, its file
    const r = reqs.find((x) => x.key === 'GW-TECH-001');
    await select(page, { kind: 'node', id: r.id });
    const box = page.locator('#g-detail');
    await expect(box.locator('.g-rkey')).toHaveText('GW-TECH-001');
    await expect(box.locator('h2')).toContainText('p99 under 200 ms');
    await expect(box.locator('.g-rst')).toHaveText(r.status);
    await expect(box.locator('.g-rtext')).toContainText('rolling hour');
    await expect(box.locator('.g-rinfile a.node')).toHaveText(g.nodes.find((n) => n.id === r.file).name);
    await expect(box.locator('.actions [data-act]')).toHaveText(['Edit…', '+ Requirement after', 'Move to…', 'Delete…']);
    await shot(page, 'req-node-detail');
    // the sections toggle hides them again
    await page.locator('#g-reqsec').uncheck();
    await expect.poll(() => page.evaluate(() => IterGraph.api.cy.nodes('.req').length)).toBe(0);
  });

  test('drag a contains line\'s file end onto another requirements file → moved', async ({ page, admin }) => {
    const id = ids.req['LED-TECH-002'];
    await openTab(page, 'graph', PROJ);
    await showReqNodes(page);
    await pickContains(page, ids.ledger_techreq, id, ids.techreq);
    await expect(page.locator('#g-detail')).toContainText('file holds requirement');
    await shot(page, 'req-contains-handles');
    const req = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith('/graph/edges/move'));
    await dragHandle(page, 'graph', 'source', ids.techreq);
    expect(JSON.parse((await req).postData())).toEqual({ from: ids.ledger_techreq, to: id, kind: 'contains', new_from: ids.techreq });
    await expect.poll(() => fileOfReq(admin, id)).toBe(ids.techreq);
    await expect.poll(() => page.evaluate((eid) => IterGraph.api.cy.getElementById(eid).nonempty(), `e:${ids.techreq}|contains|${id}`)).toBe(true);
  });

  test('dropping a contains line on a node that is not a requirements file is refused', async ({ page, admin }) => {
    const id = ids.req['LED-TECH-001'];
    await openTab(page, 'graph', PROJ);
    await showReqNodes(page);
    await pickContains(page, ids.ledger_techreq, id, ids.ledger);
    const moves = []; page.on('request', (r) => { if (r.url().includes('/graph/edges/move') || r.url().includes('/move')) moves.push(r.url()); });
    await dragHandle(page, 'graph', 'source', ids.ledger);
    await expect(page.locator('.kit-toast')).toContainText('Drop it on a bizreq or techreq file');
    await page.waitForTimeout(300);
    expect(moves).toEqual([]);
    expect(await fileOfReq(admin, id)).toBe(ids.ledger_techreq);
  });

  test('a code node shows its two files; Add creates the missing one (node + type)', async ({ page, admin }) => {
    await openTab(page, 'graph', PROJ);
    // the ledger: a techreq file, no bizreq file yet
    await select(page, { kind: 'node', id: ids.ledger });
    const box = page.locator('#g-detail');
    await expect(box.locator('.g-rfile.ft-techreq .g-count')).toHaveText(String((await reqsOf(admin, ids.ledger_techreq)).length));
    await expect(box.locator('.g-rfile.ft-bizreq')).toContainText('no file yet');
    await shot(page, 'req-code-files');
    await box.locator('[data-rfadd=bizreq]').click();
    await expect(dialog(page)).toContainText('Creates');
    await dialog(page).locator('#rq-key').fill('LED-BIZ-001');
    await dialog(page).locator('#rq-title').fill('Finance can export the ledger');
    await dialog(page).locator('#rq-text').fill('A CSV export of any day\'s entries, for the accountants.');
    const resp = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/graph/reqs'));
    await dialog(page).locator('.kit-primary').click();
    expect(JSON.parse((await resp).request().postData())).toEqual({ node: ids.ledger, type: 'bizreq', key: 'LED-BIZ-001', title: 'Finance can export the ledger', text: 'A CSV export of any day\'s entries, for the accountants.', status: 'draft' });
    const out = await (await resp).json();
    const g = await admin.graph(PROJ);
    const file = g.nodes.find((n) => n.nodetype === 'bizreq' && g.edges.some((e) => e.from === ids.ledger && e.kind === 'reqs' && e.to === n.id));
    expect(file).toBeTruthy();
    expect(out.req.file).toBe(file.id);
    await expect(box.locator('.g-rfile.ft-bizreq .g-count')).toHaveText('1');
  });
});
