// On/off switches while work drains (2026-10-22): a project or an engine
// switched off finishes the work it is running first, and both say so with
// "Draining…" until it has; an engine shows Stopped once it reports no
// running work.
const { test, expect, shot } = require('../lib/fixtures');

test.describe('switches', () => {
  test('a draining project and a draining engine both say Draining…', async ({ page, admin, engine }) => {
    await admin.put('/api/projects/swdrain', { state: 'Draining' });
    // the engine is switched off but its last heartbeat still reports running work
    await engine.post('/api/engines/mbp/heartbeat', { state: 'Running', running: 2, projects: [] });
    await admin.post('/api/switch', { target: 'iter_engine:mbp', active: false });
    try {
      await page.goto('/#tab=queue&p=swdrain'); // an empty project: no tiles to wait for
      const proj = page.locator('.sw[data-psw="swdrain"]');
      await expect(proj.locator('button.Draining')).toHaveText('Draining…');
      await expect(proj.locator('button.Stopped')).toHaveText('force');
      const eng = page.locator('.sw[data-sw="iter_engine:mbp"]');
      await expect(eng.locator('button.Draining')).toHaveText('Draining…');
      await expect(eng).toHaveAttribute('title', /finishing 2 running items/);
      await expect(eng.locator('button.Running')).toBeEnabled();
      await shot(page, 'switches-draining');

      // nothing running any more: the engine shows Stopped
      await engine.post('/api/engines/mbp/heartbeat', { state: 'Running', running: 0, projects: [] });
      await page.reload();
      await expect(eng.locator('button.Draining')).toHaveCount(0);
      await expect(eng.locator('button.Stopped.on')).toHaveText('Stopped');
    } finally {
      await admin.post('/api/switch', { target: 'iter_engine:mbp', active: true });
      await admin.del('/api/projects/swdrain');
    }
  });
});
