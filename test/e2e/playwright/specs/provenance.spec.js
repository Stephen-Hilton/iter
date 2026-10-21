// Who filed a work item is the server's to say (MG-14): the webui sends no
// requestedby / createdby when it creates or clones an item, and the detail
// line shows what the server stored.
const { test, expect, openTab, shot } = require('../lib/fixtures');

const isCreate = (r) => r.method() === 'POST' && /\/api\/projects\/shop\/workitems$/.test(new URL(r.url()).pathname);

test.describe('provenance', () => {
  test('create and clone send none; the detail shows the server\'s', async ({ page, admin }) => {
    await openTab(page, 'queue', 'shop');
    await page.click('#newitem');
    await page.fill('#f-name', 'provenance check');
    await page.fill('#f-req', 'Nothing to do: this item checks who filed it.');
    const [create] = await Promise.all([page.waitForRequest(isCreate), page.click('#f-save')]);
    const sent = create.postDataJSON();
    expect(sent).not.toHaveProperty('requestedby');
    expect(sent).not.toHaveProperty('createdby');
    const id = (await (await create.response()).json()).id;
    const stored = await admin.get(`/api/projects/shop/workitems/${id}`);
    expect(stored.requestedby).toBe('user:admin');

    // clone it from its Actions menu
    await page.selectOption('#sort', 'state');
    await page.click(`#items .tile[data-id="${id}"] .actbtn`);
    const [clone] = await Promise.all([page.waitForRequest(isCreate), page.click('#actmenu button[data-act=clone]')]);
    const cloned = clone.postDataJSON();
    expect(cloned).not.toHaveProperty('requestedby');
    expect(cloned).not.toHaveProperty('createdby');

    // the detail line names the server's values
    await page.click(`#items .tile[data-id="${id}"] .nm`);
    const dlg = page.locator('dialog#detail[open]');
    await expect(dlg).toContainText('requested by user:admin');
    await shot(page, 'provenance-detail');
    await page.click('#dlg-close');
    for (const del of [id, (await (await clone.response()).json()).id]) await admin.del(`/api/projects/shop/workitems/${del}`);
  });
});
