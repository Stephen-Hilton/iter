// Sign in with the login form, sign out again; a bad password is refused.
const { test, expect, Api } = require('../lib/fixtures');

test.describe('login / logout', () => {
  test.use({ signedIn: false });

  test('sign in, then sign out', async ({ page, state }) => {
    await page.goto('/');
    await expect(page.locator('#login')).toBeVisible();
    await expect(page.locator('#top')).toBeHidden();
    await page.fill('#li-user', 'admin');
    await page.fill('#li-pass', state.adminPassword);
    await page.click('#li-go');
    await expect(page.locator('#top')).toBeVisible();
    await expect(page.locator('#who')).toContainText('admin');
    // the token the page keeps is a real one
    const token = await page.evaluate(() => localStorage.getItem('iter.token'));
    expect(token).toBeTruthy();
    const projects = await new Api(state.base, token).get('/api/projects');
    expect(projects.map((p) => p.name)).toContain('shop');
    await page.click('#logout');
    await expect(page.locator('#login')).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem('iter.token'))).toBeNull();
  });

  test.describe('a password set by an admin', () => {
    test.use({ allowConsole: [/40[03]/] });
    test('must be changed at the first sign-in, and the change needs the current one', async ({ page, state, admin }) => {
      const who = 'pwnew' + Date.now().toString(36);
      await admin.put(`/api/users/${who}`, { role: 'user', password: 'set-by-admin-123', email: '', settings: {}, authz: {} });
      await page.goto('/');
      await page.fill('#li-user', who);
      await page.fill('#li-pass', 'set-by-admin-123');
      await page.click('#li-go');
      await expect(page.locator('#pwchange')).toBeVisible();
      await expect(page.locator('#top')).toBeHidden();
      // until it is changed, the session is good for nothing else
      const held = await page.evaluate(() => localStorage.getItem('iter.token'));
      expect((await new Api(state.base, held).call('GET', '/api/projects', undefined, [403])).status).toBe(403);
      // too short, then mismatched, then right
      await page.fill('#pc-new', 'short');
      await page.fill('#pc-new2', 'short');
      await page.click('#pc-go');
      await expect(page.locator('#pc-err')).toContainText('12');
      await page.fill('#pc-new', 'my-own-password-1');
      await page.fill('#pc-new2', 'my-own-password-2');
      await page.click('#pc-go');
      await expect(page.locator('#pc-err')).toContainText('differ');
      await page.fill('#pc-new2', 'my-own-password-1');
      await page.click('#pc-go');
      await expect(page.locator('#top')).toBeVisible();
      await expect(page.locator('#pwchange')).toBeHidden();
      // the old session ended; the new password signs in with no change asked
      expect((await new Api(state.base, held).call('GET', '/api/projects', undefined, [401])).status).toBe(401);
      const again = await Api.login(state.base, who, 'my-own-password-1');
      expect(again.mustChange).toBe(false);
    });
  });

  test.describe('bad password', () => {
    test.use({ allowConsole: [/401/] });
    test('is refused with a message', async ({ page }) => {
      await page.goto('/');
      await page.fill('#li-user', 'admin');
      await page.fill('#li-pass', 'wrong-password');
      await page.click('#li-go');
      await expect(page.locator('#li-err')).not.toBeEmpty();
      await expect(page.locator('#top')).toBeHidden();
      expect(await page.evaluate(() => localStorage.getItem('iter.token'))).toBeNull();
    });
  });
});
