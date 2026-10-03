// Settings graph: renders the seeded system (engine, account, provider, project
// and their edges); configure a node and an edge; add an edge by drawing; drag an
// edge end onto a deactivated placeholder (edge kept, inactive); tag an edge;
// copy an edge (with its settings) onto another node.
const { test, expect, openTab, posOf, dragHandle, still, dialog, shot } = require('../lib/fixtures');

const sg = (admin) => admin.get('/api/settings/graph');
const edgeOf = (g, type, from, to) => g.edges.find((e) => e.type === type && e.from === from && (!to || e.to === to));
const focus = async (page, id) => { await page.evaluate((i) => IterSettings.focus(i), id); await still(page, 'settings'); };
const tapEdge = async (page, id) => { await page.evaluate((i) => { IterSettings.cy.getElementById('se:' + i).emit('tap'); }, id); await still(page, 'settings'); };
const field = (page, key) => dialog(page).locator(`.kit-kv:has(.kit-k span[title="${key}"]) .kit-v :is(input,select,textarea)`).first();

test.describe('settings graph', () => {
  test('renders nodes and edges of the system', async ({ page, admin }) => {
    const g = await sg(admin);
    await openTab(page, 'settings');
    const drawn = await page.evaluate(() => IterSettings.cy.elements().map((e) => e.id()));
    for (const id of ['iter_data:self', 'iter_engine:mbp', 'account:main', 'provider:mock', 'provider:claude', 'project:shop', 'user:admin', 'user:pwbot']) expect(drawn).toContain(id);
    for (const [t, f, to] of [['serves', 'iter_engine:mbp', 'project:shop'], ['holds', 'iter_engine:mbp', 'account:main'], ['bills', 'account:main', 'project:shop'], ['of', 'account:main', 'provider:mock'], ['owns', 'user:pwbot', 'iter_engine:mbp']]) {
      const e = edgeOf(g, t, f, to);
      expect(e, `${t} ${f} → ${to}`).toBeTruthy();
      expect(drawn).toContain('se:' + e.id);
    }
    // tagged edges show the tag as their label
    const bills = edgeOf(g, 'bills', 'account:main', 'project:shop');
    expect(await page.evaluate((id) => IterSettings.cy.getElementById('se:' + id).data('label'), bills.id)).toBe(bills.tag);
    await expect(page.locator('#s-summary')).toContainText('nodes');
    await shot(page, 'settings-graph');
    // untagged edges hide their type label at overview zoom; hovering a node shows its edges' labels
    const serves = edgeOf(g, 'serves', 'iter_engine:mbp', 'project:shop');
    const shown = () => page.evaluate((id) => IterSettings.cy.getElementById('se:' + id).pstyle('text-opacity').value, serves.id).then(Number);
    expect(await shown()).toBe(0);
    await page.evaluate(() => IterSettings.cy.getElementById('iter_engine:mbp').emit('mouseover'));
    expect(await shown()).toBe(1);
    await shot(page, 'settings-graph-hover');
  });

  test('zoomed in, untagged edges show their type label', async ({ page, admin }) => {
    // its own page: chaining a hover and a zoom on one headless page can crash Chromium's renderer
    const serves = edgeOf(await sg(admin), 'serves', 'iter_engine:mbp', 'project:shop');
    await openTab(page, 'settings');
    const shown = () => page.evaluate((id) => IterSettings.cy.getElementById('se:' + id).pstyle('text-opacity').value, serves.id).then(Number);
    expect(await shown()).toBe(0);
    await page.evaluate(() => IterSettings.cy.zoom(2));
    await expect.poll(shown).toBe(1);
  });

  test('configure a node: a setting edited lands on the record', async ({ page, admin }) => {
    await openTab(page, 'settings');
    await focus(page, 'account:main');
    await expect(page.locator('#s-detail h2')).toHaveText('main');
    await page.keyboard.press('e');
    await expect(dialog(page)).toBeVisible();
    await shot(page, 'settings-configure-node');
    await field(page, 'token_envar').fill('CLAUDE_TOKEN_TEAM');
    await dialog(page).locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    await expect.poll(async () => (await admin.get('/api/settings/nodes/account:main')).settings.token_envar).toBe('CLAUDE_TOKEN_TEAM');
    await admin.patch('/api/settings/nodes/account:main', { settings: { token_envar: 'CLAUDE_TOKEN_MAIN' } });
  });

  test('configure an edge: edge settings + tag', async ({ page, admin }) => {
    const serves = edgeOf(await sg(admin), 'serves', 'iter_engine:mbp', 'project:shop');
    await openTab(page, 'settings');
    await tapEdge(page, serves.id);
    await expect(page.locator('#s-detail h2')).toContainText('serves');
    await page.locator('#s-detail [data-act=configure]').click();
    await expect(dialog(page)).toBeVisible();
    await shot(page, 'settings-configure-edge');
    await field(page, '__tag').fill('shop_default');
    await dialog(page).locator('.kit-kv:has(.kit-k span[title="read_only"]) .kit-switch').click();
    await dialog(page).locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    await expect.poll(async () => { const e = (await sg(admin)).edges.find((x) => x.id === serves.id); return [e.tag, e.settings.read_only, e.settings.topdir]; }).toEqual(['shop_default', true, '/tmp/iter5_pw/shop']);
    await admin.patch(`/api/settings/edges/${serves.id}`, { settings: { read_only: false } });
  });

  test('add an edge by drawing, then drag its end onto the placeholder (inactive, settings kept)', async ({ page, admin }) => {
    await openTab(page, 'settings');
    await page.check('#s-ph');
    await still(page, 'settings');
    await focus(page, 'user:admin');
    await page.keyboard.press('c');
    await expect(page.locator('#s-banner')).toContainText('Connecting from');
    await page.evaluate(() => { IterSettings.cy.fit(undefined, 40); });
    await still(page, 'settings');
    const t = await posOf(page, 'settings', 'project:shop');
    await page.mouse.click(t.x, t.y);
    await expect(dialog(page)).toContainText('member');
    await dialog(page).locator('input[type=text]').fill('admin_shop');
    await dialog(page).locator('.kit-primary').click();
    // a member edge has settings (role): Configure opens next
    await expect(dialog(page)).toContainText('role');
    await field(page, 'role').fill('viewer');
    await dialog(page).locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    let member;
    await expect.poll(async () => { member = edgeOf(await sg(admin), 'member', 'user:admin', 'project:shop'); return member && [member.tag, member.settings.role, member.active]; }).toEqual(['admin_shop', 'viewer', true]);
    // drag its project end onto the project placeholder
    await page.evaluate(() => { IterSettings.cy.fit(undefined, 40); });
    await tapEdge(page, member.id);
    await expect.poll(() => page.evaluate(() => IterSettings.cy.getElementById('__kit_ep_t').nonempty())).toBe(true);
    await shot(page, 'settings-edge-handles');
    await dragHandle(page, 'settings', 'target', 'project:_deactivated');
    await expect.poll(async () => { const e = (await sg(admin)).edges.find((x) => x.id === member.id); return e && [e.to, e.active, e.settings.role, e.tag]; }).toEqual(['project:_deactivated', false, 'viewer', 'admin_shop']);
    await shot(page, 'settings-edge-deactivated');
    // and back: active again, same settings
    await page.evaluate(() => { IterSettings.cy.fit(undefined, 40); });
    await tapEdge(page, member.id);
    await expect.poll(() => page.evaluate(() => IterSettings.cy.getElementById('__kit_ep_t').nonempty())).toBe(true);
    await dragHandle(page, 'settings', 'target', 'project:shop');
    await expect.poll(async () => { const e = (await sg(admin)).edges.find((x) => x.id === member.id); return e && [e.to, e.active, e.settings.role]; }).toEqual(['project:shop', true, 'viewer']);
    await admin.del(`/api/settings/edges/${member.id}`);
  });

  test('tag an edge from its detail pane', async ({ page, admin }) => {
    const owns = edgeOf(await sg(admin), 'owns', 'user:pwbot', 'iter_engine:mbp');
    await openTab(page, 'settings');
    await tapEdge(page, owns.id);
    await page.locator('#s-detail [data-act=tag]').click();
    await dialog(page).locator('input[type=text]').fill('bot_owns_mbp');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => (await sg(admin)).edges.find((x) => x.id === owns.id).tag).toBe('bot_owns_mbp');
    await expect.poll(() => page.evaluate((id) => IterSettings.cy.getElementById('se:' + id).data('label'), owns.id)).toBe('bot_owns_mbp');
  });

  test('create an engine node, then copy an edge onto it (settings and tag come along)', async ({ page, admin }) => {
    const holds = edgeOf(await sg(admin), 'holds', 'iter_engine:mbp', 'account:main');
    await openTab(page, 'settings');
    await page.locator('#s-editbar [data-ed=new]').click();
    await dialog(page).locator('#sn-type').selectOption('iter_engine');
    await dialog(page).locator('#sn-name').fill('mini');
    await shot(page, 'settings-new-node');
    await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => (await sg(admin)).nodes.some((n) => n.id === 'iter_engine:mini')).toBe(true);
    await still(page, 'settings');
    await tapEdge(page, holds.id);
    await page.keyboard.press('ControlOrMeta+c');
    await focus(page, 'iter_engine:mini');
    await expect(page.locator('#s-editbar [data-ed=paste]')).toBeEnabled();
    await page.keyboard.press('ControlOrMeta+v');
    if (await dialog(page).count()) await dialog(page).locator('.kit-primary').click();
    await expect.poll(async () => { const e = edgeOf(await sg(admin), 'holds', 'iter_engine:mini', 'account:main'); return e && [e.tag, e.settings.token_envar]; }).toEqual([holds.tag, holds.settings.token_envar]);
    await shot(page, 'settings-after-paste');
  });
});
