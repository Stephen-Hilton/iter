// Global setup: build iter_data (debug), create a throwaway ArangoDB database
// iter5_pw_<ts>, start iter_data on a random port serving ../../webui, seed it
// through the API, and hand the base URL + seed ids to the specs (env + state file).
'use strict';
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const net = require('net');
const path = require('path');
const { Api } = require('./lib/api');
const { seedSystem, seedProject } = require('./lib/seed');

const ROOT = path.resolve(__dirname, '../..');
const STATE = path.join(__dirname, '.state.json');
const ARANGO = process.env.ITER5_TEST_ARANGO_URL || 'http://127.0.0.1:8529';
const ARANGO_PW = process.env.ITER5_TEST_ARANGO_PASSWORD || 'iter4dev';
const ADMIN_PW = 'pw-admin-' + Math.random().toString(36).slice(2, 10);

const freePort = () => new Promise((res, rej) => {
  const s = net.createServer();
  s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => res(p)); });
  s.on('error', rej);
});

async function waitHealthy(base, child, logFile) {
  const t0 = Date.now();
  while (Date.now() - t0 < 60000) {
    if (child.exitCode != null) throw new Error(`iter_data exited (${child.exitCode}); see ${logFile}`);
    try { const r = await fetch(base + '/health'); if (r.ok) return; } catch (e) { /* not yet */ }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error('iter_data did not answer /health within 60s; see ' + logFile);
}

module.exports = async () => {
  if (!process.env.PW_SKIP_BUILD) {
    const cargo = path.join(process.env.HOME, '.cargo/bin/cargo');
    execFileSync(fs.existsSync(cargo) ? cargo : 'cargo', ['build', '-p', 'iter_data'], { cwd: ROOT, stdio: 'inherit' });
  }
  const db = `iter5_pw_${Math.floor(Date.now() / 1000)}_${process.pid}`;
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const tmp = fs.mkdtempSync(path.join(require('os').tmpdir(), 'iter5pw-'));
  const logFile = path.join(tmp, 'iter_data.log');
  const log = fs.openSync(logFile, 'w');
  const child = spawn(path.join(ROOT, 'target/debug/iter_data'), [
    '--arango-url', ARANGO, '--arango-db', db, '--listen', `127.0.0.1:${port}`,
    '--webui-dir', path.join(ROOT, 'webui'), '--secret-file', path.join(tmp, 'secret'), '--env-file', path.join(tmp, 'none.env'),
  ], {
    env: Object.assign({}, process.env, { ARANGO_URL: ARANGO, ARANGO_DB: db, ARANGO_USER: 'root', ARANGO_PASSWORD: ARANGO_PW, ITER_ADMIN_PASSWORD: ADMIN_PW, ITER_PORT: '' }),
    stdio: ['ignore', log, log], detached: false,
  });
  const state = { db, port, base, pid: child.pid, tmp, logFile, adminPassword: ADMIN_PW, arango: ARANGO };
  fs.writeFileSync(STATE, JSON.stringify(state, null, 2));
  await waitHealthy(base, child, logFile);

  const { api: admin, token } = await Api.login(base, 'admin', ADMIN_PW);
  const sys = await seedSystem(admin);
  const shop = await seedProject(admin, sys.eng, 'shop');
  Object.assign(state, { adminToken: token, engineToken: sys.engineToken, shop: { ids: shop.ids, items: shop.items } });
  fs.writeFileSync(STATE, JSON.stringify(state, null, 2));
  process.env.PW_BASE = base;
  child.unref();
};
