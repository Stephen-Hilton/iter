// Every tab loads with no console errors, screenshots of each (desktop + phone),
// and the queue has no horizontal scroll at phone width.
const { test, expect, openTab, shot } = require('../lib/fixtures');

test.describe('tabs', () => {
  for (const tab of ['queue', 'graph', 'settings', 'intro']) {
    test(`${tab} tab loads cleanly`, async ({ page }) => {
      await openTab(page, tab, 'shop');
      await expect(page.locator(`#tabs button[data-tab=${tab}]`)).toHaveClass(/on/);
      if (tab === 'intro') await expect(page.locator('#tab-intro')).not.toBeEmpty();
      await shot(page, `tab-${tab}`);
    });
  }

  // GraphRAG is hidden until its replacement (2026-11-11, RAG_TAB_HIDDEN in index.html):
  // no button, and a #tab=rag link, a saved tab or a number key never open it
  test('the GraphRAG tab is hidden and cannot be reached', async ({ page }) => {
    await openTab(page, 'queue', 'shop');
    await expect(page.locator('#tabs button[data-tab=rag]')).toBeHidden();
    await page.evaluate(() => { location.hash = '#tab=rag'; });
    await page.waitForTimeout(300);
    await expect(page.locator('#tabs button[data-tab=queue]')).toHaveClass(/on/);
    await expect(page.locator('#tab-rag')).toBeEmpty();
    // a saved tab is read only when the link names none
    await page.evaluate(() => localStorage.setItem('iter.tab', 'rag'));
    await page.goto('/');
    await page.waitForSelector('#top', { state: 'visible' });
    await expect(page.locator('#tabs button[data-tab=queue]')).toHaveClass(/on/);
    await page.evaluate(() => document.activeElement && document.activeElement.blur());
    await page.keyboard.press('4');
    await expect(page.locator('#tabs button[data-tab=settings]')).toHaveClass(/on/);
    await expect(page.locator('#tab-rag')).toBeEmpty();
  });

  test('phone width: queue has no horizontal scroll', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 800 });
    await openTab(page, 'queue', 'shop');
    const [sw, cw] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
    expect(sw).toBeLessThanOrEqual(cw);
    await shot(page, 'phone-queue', { fullPage: true });
    // the header is two rows: tabs, then project / engine status / Running|Stopped / ⋯
    const hh = await page.locator('#brandrow').evaluate((el) => el.getBoundingClientRect().height);
    expect(hh, 'phone header height').toBeLessThanOrEqual(90);
    await expect(page.locator('#logout')).toBeHidden();
    await page.click('#hdrmore-btn');
    await expect(page.locator('#logout')).toBeVisible();
    await expect(page.locator('#who')).toContainText('admin');
    await shot(page, 'phone-header-menu');
    await page.keyboard.press('Escape');
    await expect(page.locator('#logout')).toBeHidden();
    for (const tab of ['graph', 'settings', 'intro']) {
      await page.click(`#tabs button[data-tab=${tab}]`);
      await page.waitForTimeout(900);
      const [sw2, cw2] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
      expect(sw2, `${tab} at 375px`).toBeLessThanOrEqual(cw2);
      await shot(page, `phone-${tab}`);
    }
  });
});
