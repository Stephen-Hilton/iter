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
    const token = await page.evaluate(() => localStorage.getItem('iter5.token'));
    expect(token).toBeTruthy();
    const projects = await new Api(state.base, token).get('/api/projects');
    expect(projects.map((p) => p.name)).toContain('shop');
    await page.click('#logout');
    await expect(page.locator('#login')).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem('iter5.token'))).toBeNull();
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
      expect(await page.evaluate(() => localStorage.getItem('iter5.token'))).toBeNull();
    });
  });
});
