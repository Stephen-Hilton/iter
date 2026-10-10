// The work queue renders every item, an item opens, and a state change made in
// the menu lands on the server.
const { test, expect, openTab, shot } = require('../lib/fixtures');

test.describe('work queue', () => {
  test('renders every item of the project (by state)', async ({ page, admin }) => {
    const items = await admin.get('/api/projects/shop/workitems');
    await openTab(page, 'queue', 'shop');
    await page.selectOption('#sort', 'state');
    await expect(page.locator('#items .tile')).toHaveCount(items.length);
    for (const i of items) await expect(page.locator(`#items .tile[data-id="${i.id}"] .nm`)).toHaveText(i.name);
    await shot(page, 'queue-by-state');
  });

  test('open an item: the detail dialog shows it', async ({ page, state, admin }) => {
    const id = state.shop.items.question;
    const item = await admin.get(`/api/projects/shop/workitems/${id}`);
    await openTab(page, 'queue', 'shop');
    await page.selectOption('#sort', 'state');
    await page.click(`#items .tile[data-id="${id}"] .nm`);
    const dlg = page.locator('dialog#detail[open]');
    await expect(dlg).toBeVisible();
    await expect(dlg).toContainText(item.name);
    await expect(dlg).toContainText('question');
    await shot(page, 'queue-detail');
    await page.click('#dlg-close');
    await expect(page.locator('dialog#detail[open]')).toHaveCount(0);
  });

  test('park a queued item from its Actions menu', async ({ page, state, admin }) => {
    const id = state.shop.items.queued;
    await openTab(page, 'queue', 'shop');
    await page.selectOption('#sort', 'state');
    await page.click(`#items .tile[data-id="${id}"] .actbtn`);
    await expect(page.locator('#actmenu')).toBeVisible();
    await shot(page, 'queue-actions');
    await page.click('#actmenu button[data-act=park]');
    await expect.poll(async () => (await admin.get(`/api/projects/shop/workitems/${id}`)).state).toBe('parked');
    await expect(page.locator(`#items .tile[data-id="${id}"] .state`)).toHaveText('parked');
    // and back, so the other specs see the seed
    await page.click(`#items .tile[data-id="${id}"] .actbtn`);
    await page.click('#actmenu button[data-act=queue]');
    await expect.poll(async () => (await admin.get(`/api/projects/shop/workitems/${id}`)).state).toBe('queued');
  });
});
