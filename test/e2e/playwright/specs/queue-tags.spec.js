// The queue line's tag budget (2026-10-20): a tag-heavy item shows only its most relevant
// tags (sweep > blockers > use cases > the rest) and a "+N more ⬇" chip whose popup lists
// them all; the repeat key (check: / container:) is folded off the tags at create.
const { test, expect, openTab, shot } = require('../lib/fixtures');

test.describe('queue tags', () => {
  test('a tag-heavy item: ranked, capped, "+N more" popup lists every tag', async ({ page, admin }) => {
    const ucs = ['01_setup_new_employer.demo', '02_setup_new_cmp.demo', '03_stream_employee_wages.demo', 'auditor_join', 'payroll_close'];
    const item = await admin.post('/api/projects/shop/workitems', {
      name: 'Test coverage: top up the tests for Core Product Services', agent: 'test', priority: 10, requestedby: 'user',
      request: 'Bring the tests of the core services up to their coverage.',
      tags: [{ text: 'check:test-coverage' }, { text: 'container:m17' }, ...ucs.map((u) => ({ text: `usecase:${u}` })), { text: 'human-note' }, { text: 'sweep' }],
    });
    expect(item.repeat_check).toBe('test-coverage');
    expect(item.repeat_container).toBe('m17');
    expect(item.tags.map((t) => t.text)).not.toContain('container:m17');

    await openTab(page, 'queue', 'shop');
    await page.selectOption('#sort', 'state');
    const tile = page.locator(`#items .tile[data-id="${item.id}"]`);
    await expect(tile).toBeVisible();
    const tags = tile.locator('.tags .tag:not(.tagmore)');
    await expect(tags.first()).toHaveText('sweep'); // sweeps rank first
    expect(await tags.count()).toBeLessThanOrEqual(3);
    const more = tile.locator('.tagmore');
    await expect(more).toContainText('more ⬇');
    await expect(tile).not.toContainText('container:');
    // the tags stay inside the tile: the line is readable
    const tb = await tile.boundingBox(), rb = await tile.locator('.tags').boundingBox();
    expect(rb.x).toBeGreaterThan(tb.x + tb.width / 3);

    await more.click();
    const pop = page.locator('#tagpop');
    await expect(pop).toBeVisible();
    await expect(pop.locator('.tag')).toHaveCount(ucs.length + 2);
    for (const u of ucs) await expect(pop).toContainText(u);
    await expect(page.locator('dialog#detail[open]')).toHaveCount(0); // the chip does not open the item
    await shot(page, 'queue-tags-more');
    await page.keyboard.press('Escape');
    await expect(pop).toBeHidden();
  });
});
