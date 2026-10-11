// Engine keys (admins): add an engine's public key, sign in with its private
// key, rotate it (the old one stops), revoke and delete — all through the 🔑
// screen, checked against /auth/engine with real Ed25519 keys.
const crypto = require('crypto');
const { test, expect, shot, Api } = require('../lib/fixtures');

// what `iter_engine key new` makes (iter_core::enginekey)
const newKey = () => {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  return { privateKey, pub: 'iter_' + raw.toString('hex'), id: crypto.createHash('sha256').update(raw).digest('hex').slice(0, 12) };
};
// the engine's signed login: 200 with a session, or the refusal
const signsIn = async (state, k) => {
  const ts = Math.floor(Date.now() / 1000), nonce = crypto.randomBytes(16).toString('hex');
  const signature = crypto.sign(null, Buffer.from(`iter-engine-login\n${k.id}\n${ts}\n${nonce}`), k.privateKey).toString('hex');
  const r = await new Api(state.base).call('POST', '/auth/engine', { key_id: k.id, ts, nonce, signature }, [401]);
  return r.status === 200;
};
const keys = (page) => page.locator('#settings[open] .keys');
const rowOf = (page, id) => keys(page).locator('tr', { has: page.locator('td.kid', { hasText: id }) });

test.describe('engine keys', () => {
  // the malformed key is refused (400) on purpose
  test.use({ allowConsole: [/status of 400/] });
  test('add, sign in, rotate, revoke and delete an engine key', async ({ page, state }) => {
    const engine = 'pwkey' + crypto.randomBytes(3).toString('hex');
    const k1 = newKey(), k2 = newKey();
    expect(await signsIn(state, k1)).toBe(false);

    await page.goto('/');
    await page.locator('#keys-btn').click();
    await expect(keys(page)).toBeVisible();
    // a malformed key is refused with the server's reason
    await keys(page).locator('#k-pub').fill('not-a-key');
    await keys(page).locator('#k-eng').fill(engine);
    await keys(page).locator('#k-add').click();
    await expect(keys(page).locator('#sf-err')).toContainText('not an engine public key');

    await keys(page).locator('#k-pub').fill(k1.pub);
    await keys(page).locator('#k-eng').fill(engine);
    await keys(page).locator('#k-label').fill('attic laptop');
    await keys(page).locator('#k-add').click();
    await expect(rowOf(page, k1.id)).toContainText('active');
    await expect(rowOf(page, k1.id)).toContainText('attic laptop');
    expect(await signsIn(state, k1)).toBe(true);
    // the seeded engine user (pwbot) is an older engine login, offered for removal
    await expect(keys(page)).toContainText('Older engine logins');
    await expect(keys(page).locator('[data-ksig="pwbot"]')).toBeVisible();
    await shot(page, 'keys');

    // rotate, the old one stopping now
    await rowOf(page, k1.id).locator('[data-krot]').click();
    const rot = keys(page).locator('tr.krot');
    await rot.locator('input').fill(k2.pub);
    await rot.locator('select').selectOption('0');
    await rot.locator('button', { hasText: 'rotate' }).click();
    await expect(rowOf(page, k2.id)).toContainText('active');
    await expect(rowOf(page, k1.id)).toContainText('revoked');
    expect(await signsIn(state, k1)).toBe(false);
    expect(await signsIn(state, k2)).toBe(true);

    // revoke (confirmed), then delete both
    page.once('dialog', (d) => d.accept());
    await rowOf(page, k2.id).locator('[data-krev]').click();
    await expect(rowOf(page, k2.id)).toContainText('revoked');
    expect(await signsIn(state, k2)).toBe(false);
    for (const k of [k1, k2]) {
      await rowOf(page, k.id).locator('[data-kdel]').click();
      await expect(rowOf(page, k.id)).toHaveCount(0);
    }
  });
});
