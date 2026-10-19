// One user form (2026-10-19): the gear in the header and a user node in the
// settings graph open the same form: an admin edits anyone, a person edits
// their own profile (role and project access read only), and another person's
// form is read only and shows only who they are and their role. Accounts show
// their token env var setting before it is set, and a new account asks for it.
const crypto = require('crypto');
const { test, expect, openTab, still, dialog, shot, Api } = require('../lib/fixtures');

const form = (page) => page.locator('#settings[open]');
const focus = async (page, id) => { await page.evaluate((i) => IterSettings.focus(i), id); await still(page, 'settings'); };

test.describe('user form', () => {
  test('the gear and the admin user node open the same form', async ({ page }) => {
    await page.goto('/');
    await page.locator('#user-gear').click();
    await expect(form(page).locator('h2')).toHaveText(/My settings — admin/);
    const gearFields = await form(page).locator('.frow > label').allTextContents();
    await form(page).locator('#sf-close').click();

    await openTab(page, 'settings');
    await focus(page, 'user:admin');
    await page.keyboard.press('e');
    await expect(form(page).locator('h2')).toHaveText(/My settings — admin/);
    expect(await form(page).locator('.frow > label').allTextContents()).toEqual(gearFields);
    await expect(form(page).locator('#sf-save')).toBeVisible();
    await expect(form(page).locator('#mp-go')).toBeVisible();
    await expect(dialog(page)).toHaveCount(0); // not the generic key/value editor
    await shot(page, 'userform-admin-from-graph');
  });

  test.describe('as a person who is not an admin', () => {
    test.use({ signedIn: false });
    test('their own form edits the profile; the admin\'s is read only', async ({ page, admin, state }) => {
      const who = 'pwuser' + crypto.randomBytes(3).toString('hex');
      const first = 'first-password-' + crypto.randomBytes(4).toString('hex');
      const mine = 'my-own-password-' + crypto.randomBytes(4).toString('hex');
      await admin.put('/api/users/' + who, { role: 'user', password: first, email: 'p@example.com', settings: {}, authz: {} });
      const l = await Api.login(state.base, who, first);
      const { token } = await l.api.post(`/api/users/${who}/password`, { current_password: first, new_password: mine });
      await page.addInitScript(([t, u]) => { localStorage.setItem('iter5.token', t); localStorage.setItem('iter5.user', u); localStorage.setItem('iter5.role', 'user'); }, [token, who]);

      await page.goto('/');
      await page.locator('#user-gear').click();
      await expect(form(page).locator('h2')).toHaveText(new RegExp('My settings — ' + who));
      await expect(form(page).locator('#sf-save')).toBeVisible();
      await expect(form(page).locator('#sf-role')).toHaveCount(0); // role is shown, not editable
      await form(page).locator('#sf-email').fill('new@example.com');
      await form(page).locator('#sf-save').click();
      await expect(form(page)).toHaveCount(0);
      expect((await admin.get('/api/users/' + who)).email).toBe('new@example.com');

      await page.evaluate(() => openSettings('user', 'admin'));
      await expect(form(page).locator('h2')).toHaveText(/User — admin/);
      await expect(form(page).locator('#sf-save')).toHaveCount(0);
      await expect(form(page)).toContainText('read only');
      expect(await form(page).locator('.frow > label').allTextContents()).toEqual(['id', 'name', 'role']);
      await shot(page, 'userform-someone-else');
    });
  });
});

test.describe('accounts', () => {
  test('an account shows its token env var, and a new one asks for it', async ({ page, admin }) => {
    await openTab(page, 'settings');
    const name = 'pwacct' + crypto.randomBytes(3).toString('hex');
    await page.evaluate(() => { IterSettings.newNode('account'); });
    const d = dialog(page);
    await d.locator('#sn-name').fill(name);
    await expect(d.locator('#sn-envar')).toBeVisible();
    await d.locator('#sn-envar').fill('not a name');
    await d.locator('.kit-primary').click();
    await expect(d).toContainText('variable name');
    await d.locator('#sn-envar').fill('DEV7_TOKEN');
    await d.locator('.kit-primary').click();
    await expect(dialog(page)).toHaveCount(0);
    const g = await admin.get('/api/settings/graph');
    const n = g.nodes.find((x) => x.type === 'account' && x.name === name);
    expect(n.settings.token_envar).toBe('DEV7_TOKEN');

    // an account made without one still shows the setting, empty, to fill in
    const bare = 'pwbare' + crypto.randomBytes(3).toString('hex');
    const r = await admin.post('/api/settings/nodes', { type: 'account', name: bare, settings: {} });
    await openTab(page, 'settings');
    await focus(page, r.id);
    await page.keyboard.press('e');
    await expect(dialog(page).locator('.kit-k span[title="token_envar"]')).toBeVisible();
    await shot(page, 'account-token-envar');
  });
});
