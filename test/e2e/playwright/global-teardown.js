// Global teardown: stop iter_data and drop the throwaway database.
'use strict';
const fs = require('fs');
const path = require('path');

const STATE = path.join(__dirname, '.state.json');

module.exports = async () => {
  let st;
  try { st = JSON.parse(fs.readFileSync(STATE, 'utf8')); } catch (e) { return; }
  if (st.pid) { try { process.kill(st.pid, 'SIGTERM'); } catch (e) { /* gone */ } }
  await new Promise((r) => setTimeout(r, 300));
  if (st.pid) { try { process.kill(st.pid, 'SIGKILL'); } catch (e) { /* gone */ } }
  if (st.db && /^iter5_pw_/.test(st.db) && !process.env.PW_KEEP_DB) {
    const pw = process.env.ITER5_TEST_ARANGO_PASSWORD || 'iter4dev';
    await fetch(`${st.arango}/_db/_system/_api/database/${st.db}`, { method: 'DELETE', headers: { authorization: 'Basic ' + Buffer.from('root:' + pw).toString('base64') } }).catch(() => {});
  }
  if (st.tmp && !process.env.PW_KEEP_DB) fs.rmSync(st.tmp, { recursive: true, force: true });
  fs.rmSync(STATE, { force: true });
};
