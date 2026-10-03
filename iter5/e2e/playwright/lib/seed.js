// Seed data for the Playwright suite, through the public API only (the same
// calls the webui, an engine and the wizard make):
//
//   settings graph   engine user `pwbot` (role engine) owning engine `mbp`;
//                    account `main` (of provider:mock, held by mbp, bills the project)
//   project graph    a realistic iter5 graph — project node, philosophy + reqs,
//                    two contexts, containers, components, two connection nodes
//                    (supplies / connects), test nodes with results and logs, two
//                    use cases + actors, built on `mbp` (build → files/pending →
//                    files/ack → build/done), one sync conflict, one pending edit
//   work queue       items in several states
'use strict';
const crypto = require('crypto');

const sha = (t) => crypto.createHash('sha256').update(t).digest('hex');
const enc = encodeURIComponent;

async function seedSystem(admin) {
  // the engine's own user + token (an engine authenticates as its owner)
  await admin.put('/api/users/pwbot', { user: 'pwbot', role: 'engine', email: '', password: 'pwbot-pass-123' });
  const tok = (await admin.post('/api/users/pwbot/token', { ttl_days: 2 })).token;
  const eng = admin.as(tok);
  await eng.put('/api/engines/mbp', { state: 'Running', ticksec: 5, last_seen: new Date().toISOString() });
  await admin.post('/api/settings/nodes', { type: 'account', name: 'main', settings: { provider: 'mock', token_envar: 'CLAUDE_TOKEN_MAIN' } });
  await admin.post('/api/settings/edges', { from: 'iter_engine:mbp', to: 'account:main', tag: 'mbp_main', settings: { token_envar: 'CLAUDE_TOKEN_MAIN' } });
  return { engineToken: tok, eng };
}

/**
 * A project with the full v5 graph. opts.built: build it on `mbp` and ack every
 * file (synced, served); else it stays designed.
 */
async function seedProject(admin, eng, name, opts) {
  opts = Object.assign({ built: true, conflict: true, pending: true, items: true }, opts || {});
  const P = `/api/projects/${enc(name)}`;
  await admin.put(P, { state: 'Running', desc: `${name}: an online shop that takes payments and settles them daily.` });
  const ids = {};
  const create = async (key, body) => {
    const r = await admin.post(`${P}/graph/nodes`, body);
    ids[key] = r.node.id;
    return r.node;
  };
  const g0 = await admin.graph(name);
  const proj = g0.nodes.find((n) => n.nodetype === 'project');
  ids.project = proj.id;
  ids.philosophy = g0.nodes.find((n) => n.nodetype === 'philosophy').id;
  await admin.patch(`${P}/graph/nodes/${ids.philosophy}`, { name: 'Keep it boring', desc: 'Prefer plain, proven technology; settle conflicts in favour of the simplest design that works.', body: '# Keep it boring\n\nPrefer plain, proven technology. When two requirements conflict, the simpler design wins.\n' });

  await create('storefront', { nodetype: 'code', level: 'context', name: 'Storefront', desc: 'Everything a shopper sees and touches: the web app and the API in front of it.', attach_to: ids.project, attach_kind: 'codenodes' });
  await create('payments', { nodetype: 'code', level: 'context', name: 'Payments', desc: 'Money movement: the ledger, its database and the nightly settlement.', attach_to: ids.project, attach_kind: 'codenodes' });
  await create('webapp', { nodetype: 'code', level: 'container', name: 'Web app', desc: 'The browser app shoppers use to browse and pay.', attach_to: ids.storefront, attach_kind: 'codenodes', front: { owner: 'bespoke' } });
  await create('gateway', { nodetype: 'code', level: 'container', name: 'API gateway', desc: 'Kong in front of every backend service; routes and rate-limits API calls.', attach_to: ids.storefront, attach_kind: 'codenodes', front: { owner: 'oss' } });
  await create('ledger', { nodetype: 'code', level: 'container', name: 'Ledger service', desc: 'Records every payment as balanced ledger entries.', attach_to: ids.payments, attach_kind: 'codenodes', front: { owner: 'bespoke' } });
  await create('postgres', { nodetype: 'code', level: 'container', name: 'Postgres', desc: 'The ledger database.', attach_to: ids.payments, attach_kind: 'codenodes', front: { owner: 'oss' } });
  await create('checkout', { nodetype: 'code', level: 'component', name: 'Checkout form', desc: 'Collects the card and submits the payment.', attach_to: ids.webapp, attach_kind: 'codenodes' });
  await create('settle', { nodetype: 'code', level: 'component', name: 'Settlement job', desc: 'Nightly batch that settles the day\'s payments with the bank.', attach_to: ids.ledger, attach_kind: 'codenodes' });
  // connections: a TYPE of link; suppliers point in, it connects out
  await create('http', { nodetype: 'code', level: 'connection', name: 'API call over HTTP', desc: 'Synchronous JSON over HTTPS through the gateway.', attach_to: ids.gateway, attach_kind: 'supplies' });
  await admin.post(`${P}/graph/edges`, { from: ids.http, to: ids.ledger, kind: 'connects' });
  await admin.post(`${P}/graph/edges`, { from: ids.http, to: ids.webapp, kind: 'connects' });
  await create('sql', { nodetype: 'code', level: 'connection', name: 'SQL over TCP', desc: 'Postgres wire protocol.', attach_to: ids.postgres, attach_kind: 'supplies' });
  await admin.post(`${P}/graph/edges`, { from: ids.sql, to: ids.ledger, kind: 'connects' });
  await admin.post(`${P}/graph/edges`, { from: ids.sql, to: ids.settle, kind: 'connects' });
  // requirements
  // requirements: one bizreq + one techreq file per attachment point (§2.8); a new project already has its global pair
  const globalFile = (t) => (g0.nodes.find((n) => n.nodetype === t && (g0.edges || []).some((e) => e.from === ids.project && e.kind === 'reqs' && e.to === n.id)) || {}).id;
  ids.bizreq = globalFile('bizreq');
  if (ids.bizreq) await admin.patch(`${P}/graph/nodes/${ids.bizreq}`, { desc: 'What the business needs from the shop: settlement, refunds, receipts.' });
  else await create('bizreq', { nodetype: 'bizreq', name: 'Business requirements', desc: 'What the business needs from the shop: settlement, refunds, receipts.', attach_to: ids.project, attach_kind: 'reqs' });
  await create('techreq', { nodetype: 'techreq', name: 'API gateway', desc: 'Technical requirements of the API gateway.', attach_to: ids.gateway, attach_kind: 'reqs' });
  // tests
  if (opts.reqs !== false) await seedReqs(admin, name, ids);
  await create('t_gateway', { nodetype: 'test', name: 'Gateway smoke', desc: 'Boots the gateway and calls every route once.', attach_to: ids.gateway, attach_kind: 'tests' });
  await create('t_ledger', { nodetype: 'test', name: 'Ledger contract', desc: 'Double-entry invariants on the ledger API.', attach_to: ids.ledger, attach_kind: 'tests' });
  // use cases + actors
  await create('uc_pay', { nodetype: 'usecase', name: 'Pay a bill', desc: 'A shopper pays for an order with a card.',
    front: { flowmap: { summary: 'The shopper submits the checkout form; the gateway routes it to the ledger, which records the payment.',
      sequence: ['Shopper', 'Web app', 'API gateway', 'Ledger service'],
      process_flow: [{ step: 1, from: 'Shopper', to: 'Web app', what: 'fills in the checkout form' }, { step: 2, from: 'Web app', to: 'API gateway', what: 'POST /payments' }, { step: 3, from: 'API gateway', to: 'Ledger service', what: 'routes the payment' }],
      data_flow: [{ step: 1, from: 'Ledger service', to: 'Postgres', data: 'writes two balanced entries', stored: true }] } } });
  for (const k of ['webapp', 'gateway', 'ledger']) await admin.post(`${P}/graph/edges`, { from: ids.uc_pay, to: ids[k], kind: 'uses' });
  await create('uc_settle', { nodetype: 'usecase', name: 'Settle the day', desc: 'Finance closes the day and settles with the bank.' });
  await admin.post(`${P}/graph/edges`, { from: ids.uc_settle, to: ids.settle, kind: 'uses' });
  await create('shopper', { nodetype: 'actor', name: 'Shopper', desc: 'A person buying something.', attach_to: ids.uc_pay, attach_kind: 'drives' });
  await admin.post(`${P}/graph/edges`, { from: ids.shopper, to: ids.webapp, kind: 'touches' });
  await create('clerk', { nodetype: 'actor', name: 'Finance clerk', desc: 'Closes the books each day.', attach_to: ids.uc_settle, attach_kind: 'drives' });

  const out = { name, ids };
  if (opts.built) {
    const b = await admin.post(`${P}/build`, { engine: 'mbp', topdir: `/tmp/iter5_pw/${name}`, queue_plan: false });
    out.build = b;
    // the engine writes every file and acks it
    const pend = (await eng.get(`${P}/files/pending`)).pending;
    const files = {};
    const acks = pend.map((x) => { files[x.id] = x; return { id: x.id, node_version: x.node_version, path: x.path, hash: sha(x.text || ''), commit: 'c0ffee0' }; });
    await eng.post(`${P}/files/ack`, { engine: 'mbp', acks });
    await eng.post(`${P}/build/done`, { engine: 'mbp', commit: 'c0ffee0' });
    out.files = files;

    // test results (engine posts the standard JSON); two runs of the ledger test → two log rows
    const bucket = (t, p, e) => ({ total: t, pass: p, err: e || 0 });
    await eng.post(`${P}/graph/nodes/${ids.t_gateway}/testresult`, { engine: 'mbp', exit_code: 0, result: { name: 'Gateway smoke', id: ids.t_gateway, overall_success: true, normal: bucket(12, 12), longtail: bucket(3, 3), failure: bucket(2, 2) } });
    await eng.post(`${P}/graph/nodes/${ids.t_ledger}/testresult`, { engine: 'mbp', exit_code: 0, result: { name: 'Ledger contract', id: ids.t_ledger, overall_success: true, normal: bucket(20, 20), longtail: bucket(0, 0), failure: bucket(4, 4) } });
    await eng.post(`${P}/graph/nodes/${ids.t_ledger}/testresult`, { engine: 'mbp', exit_code: 1, result: { name: 'Ledger contract', id: ids.t_ledger, overall_success: false, normal: bucket(20, 18, 1), longtail: bucket(0, 0), failure: bucket(4, 4),
      details: [{ name: 'refund balances', bucket: 'normal', pass: false, msg: 'debit 10.00 != credit 9.99' }, { name: 'capture twice', bucket: 'normal', pass: false, msg: 'second capture accepted' }, { name: 'idempotent post', bucket: 'normal', pass: true, msg: '' }] } });
    // the graph changed again (test results are file writes): ack them so the project reads synced
    const pend2 = (await eng.get(`${P}/files/pending`)).pending;
    if (pend2.length) await eng.post(`${P}/files/ack`, { engine: 'mbp', acks: pend2.map((x) => { files[x.id] = x; return { id: x.id, node_version: x.node_version, path: x.path, hash: sha(x.text || ''), commit: 'c0ffee1' }; }) });

    if (opts.conflict) {
      // a sync conflict: the node is edited here while the file changes on disk; the server's (newer) edit wins
      const before = await admin.get(`${P}/graph/nodes/${ids.postgres}`);
      const node = before.node || before;
      await admin.patch(`${P}/graph/nodes/${ids.postgres}`, { desc: 'The ledger database (Postgres 16, one primary, one replica).' });
      const f = files[ids.postgres];
      const text = f.text.replace(/\n*$/, '\n\nEdited by hand on disk.\n');
      await eng.post(`${P}/files/sync`, { engine: 'mbp', full: false, files: [{ path: f.path, text, hash: sha(text), base_version: node.file_version || node.node_version }], deleted: [] });
      const pend3 = (await eng.get(`${P}/files/pending`)).pending;
      if (pend3.length) await eng.post(`${P}/files/ack`, { engine: 'mbp', acks: pend3.map((x) => ({ id: x.id, node_version: x.node_version, path: x.path, hash: sha(x.text || ''), commit: 'c0ffee2' })) });
    }
    if (opts.pending) {
      // one edit an engine has not written yet
      await admin.patch(`${P}/graph/nodes/${ids.checkout}`, { desc: 'Collects the card (hosted fields) and submits the payment.' });
    }
  }

  if (opts.items) out.items = await seedItems(admin, name);
  // the account bills the project
  await admin.post('/api/settings/edges', { from: 'account:main', to: `project:${name}`, tag: `${name}_billing`, settings: { order: 1, switch: 80, stop: 95, model: '' } }).catch(() => {});
  return out;
}

/**
 * Requirement files hold many requirements (spec §2.8): the global bizreq file gets three sections, the
 * gateway's techreq file two, and the ledger gets a techreq file of its own (created by POST graph/reqs with
 * node + type). Through POST graph/reqs when the server has it; else the §2.8 body format through graph/nodes.
 * ids.req[KEY] = each requirement's id; ids.ledger_techreq = the ledger's file.
 */
const REQ_SEED = {
  bizreq: [
    { key: 'PAY-BIZ-001', title: 'Payments settle daily', status: 'agreed', text: 'Every captured payment is settled with the bank by **06:00 UTC** the next day.\n\nFinance closes the books each morning; a payment left unsettled holds up the close.' },
    { key: 'PAY-BIZ-002', title: 'Refunds land within five days', status: 'draft', text: 'A refund reaches the shopper\'s card within five working days of the request.\n\n### Why\nCard schemes allow longer, but support tickets double after day five.' },
    { key: 'PAY-BIZ-003', title: 'Every payment has a receipt', status: 'done', text: 'The shopper gets an emailed receipt for every captured payment, with the order number and the amount.' },
  ],
  techreq: [
    { key: 'GW-TECH-001', title: 'p99 under 200 ms', status: 'agreed', text: 'The gateway answers 99% of calls within 200 ms, measured at the edge over a rolling hour.' },
    { key: 'GW-TECH-002', title: 'Rate limits per API key', status: 'draft', text: 'Each API key gets its own token bucket: 50 requests per second, bursts of 100.' },
  ],
  ledger: [
    { key: 'LED-TECH-001', title: 'Double entry always balances', status: 'agreed', text: 'Every write is two entries, a debit and a credit of the same amount, in one transaction.' },
    { key: 'LED-TECH-002', title: 'Ledger writes are idempotent', status: 'draft', text: 'Posting the same payment twice (same idempotency key) records it once and answers the first result again.' },
  ],
};
const reqSection = (r, id) => `## ${r.key ? r.key + ' — ' : ''}${r.title}\n<!-- req: id=${id} status=${r.status} -->\n${r.text}\n`;
async function seedReqs(admin, name, ids) {
  const P = `/api/projects/${enc(name)}`;
  ids.req = {};
  const probe = await admin.call('POST', `${P}/graph/reqs`, { file: ids.bizreq, ...REQ_SEED.bizreq[0] }, [404, 405]);
  if (probe.status < 300) {
    ids.req[REQ_SEED.bizreq[0].key] = probe.body.req.id;
    for (const r of REQ_SEED.bizreq.slice(1)) ids.req[r.key] = (await admin.post(`${P}/graph/reqs`, { file: ids.bizreq, ...r })).req.id;
    for (const r of REQ_SEED.techreq) ids.req[r.key] = (await admin.post(`${P}/graph/reqs`, { file: ids.techreq, ...r })).req.id;
    for (const r of REQ_SEED.ledger) {
      const out = await admin.post(`${P}/graph/reqs`, ids.ledger_techreq ? { file: ids.ledger_techreq, ...r } : { node: ids.ledger, type: 'techreq', ...r });
      ids.req[r.key] = out.req.id;
      ids.ledger_techreq = ids.ledger_techreq || (out.file && (out.file.id || (out.file.doc && out.file.doc.id))) || out.req.file;
    }
    return;
  }
  // an iter_data without graph/reqs yet: the same sections, written as file bodies
  const body = (list) => list.map((r) => { const id = crypto.randomUUID(); ids.req[r.key] = id; return reqSection(r, id); }).join('\n');
  await admin.patch(`${P}/graph/nodes/${ids.bizreq}`, { body: body(REQ_SEED.bizreq) });
  await admin.patch(`${P}/graph/nodes/${ids.techreq}`, { body: body(REQ_SEED.techreq) });
  const led = await admin.post(`${P}/graph/nodes`, { nodetype: 'techreq', name: 'Ledger service', desc: 'Technical requirements of the ledger service.', attach_to: ids.ledger, attach_kind: 'reqs', body: body(REQ_SEED.ledger) });
  ids.ledger_techreq = led.node.id;
}

async function seedItems(admin, name) {
  const P = `/api/projects/${enc(name)}/workitems`;
  const mk = async (state, iname, extra) => {
    const r = await admin.post(P, Object.assign({ name: iname, agent: 'code', state, priority: 20, tags: [], lockdirs: [], blockedby: [], request: `${iname}.\n\nmock: say done` }, extra || {}));
    return r.id;
  };
  const items = {};
  items.queued = await mk('queued', 'Add refunds to the ledger API');
  items.queued2 = await mk('queued', 'Rate-limit the checkout endpoint', { priority: 12 });
  items.question = await mk('question', 'Pick a settlement bank file format');
  items.paused = await mk('paused', 'Migrate Postgres to 16');
  items.parked = await mk('parked', 'Spike: event sourcing for the ledger');
  items.failed = await mk('failed', 'Fix flaky gateway smoke test', { agent: 'test' });
  items.complete = await mk('complete', 'Design the checkout form', { agent: 'plan' });
  return items;
}

module.exports = { seedSystem, seedProject, seedItems, seedReqs, REQ_SEED, sha };
