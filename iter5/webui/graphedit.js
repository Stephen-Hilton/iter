/*
 * iter5 Project graph editor (spec §3.3, §6.2, §9). Nodes are files: every
 * edit here goes straight to iter_data, shows in the drawing at once, and an
 * engine serving the project writes the file within seconds (the node shows
 * "⟳ pending sync" until it has). A project with no repository yet is
 * "designed": its nodes exist only here until Build creates the repository.
 *
 *   toolbar  sync state · + Node · Connect · Paste edge · Run tests
 *   node     right-click / long-press / ⌘-click: Configure… · Add node here… · Connect from here ·
 *            Paste edge here · Run tests · Move file… · Hide · Delete…
 *   edge     select it: drag either end onto another node (graph/edges/move);
 *            right-click: Configure… · Copy edge · Remove…
 *   keys     N new node · C connect · E configure · T run tests · Del delete/remove · ⌘C/⌘V copy/paste an edge
 *   reqs     a bizreq/techreq file's table (spec §2.8): add, edit (Configure), status, move to another file,
 *            delete; a code node's two files (Add creates the file); a req node's own pane; drag a
 *            "contains" line's file end onto another requirements file to move the requirement
 *
 * API (under /api/projects/{p}): GET graph · POST graph/nodes · PATCH/DELETE graph/nodes/{id} ·
 * POST graph/nodes/{id}/move · POST/DELETE graph/edges · POST graph/edges/move · POST graph/run_tests ·
 * POST graph/reqs · PATCH/DELETE graph/reqs/{id} · POST graph/reqs/{id}/move ·
 * GET/POST build. Uses IterKit (kit.js) for dialogs, menus and the edge-end handles.
 */
(function () {
  'use strict';
  const K = () => window.IterKit;
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const enc = encodeURIComponent;
  let S = null; // {root, ctx, data (normalized), canEdit}
  let DRAW = null; let keyWired = false;
  const LISTEN = { root: null, detail: (ev) => decorate(ev.detail), sel: () => { if (S) renderBar(true); } }; let pollTimer = null; let buildTimer = null; let BUILD = null;
  let CONFLICTS = []; // GET graph/conflicts rows (newest first), refreshed with the graph
  const REQ_TYPES = ['bizreq', 'techreq', 'philosophy'];
  const NEW_TYPES = [
    { key: 'context', nodetype: 'code', level: 'context', label: 'Context', hint: 'a major area of the system' },
    { key: 'container', nodetype: 'code', level: 'container', label: 'Container', hint: 'one deployable program' },
    { key: 'component', nodetype: 'code', level: 'component', label: 'Component', hint: 'a part inside a program' },
    { key: 'connection', nodetype: 'code', level: 'connection', label: 'Connection', hint: 'a type of link: API call over HTTP, event, stream, MCP…' },
    { key: 'test', nodetype: 'test', label: 'Test', hint: 'the metadata for a set of test scripts' },
    { key: 'bizreq', nodetype: 'bizreq', label: 'Business requirements', hint: 'a file of requirements, one ## section each' },
    { key: 'techreq', nodetype: 'techreq', label: 'Technical requirements', hint: 'a file of requirements, one ## section each' },
    { key: 'philosophy', nodetype: 'philosophy', label: 'Philosophy', hint: 'the highest-level guide, free prose' },
    { key: 'usecase', nodetype: 'usecase', label: 'Use case', hint: 'one journey through the product' },
    { key: 'actor', nodetype: 'actor', label: 'Actor', hint: 'who drives use cases, where people touch the app' },
  ];
  const KIND_ORDER = ['codenodes', 'tests', 'reqs', 'uses', 'drives', 'touches', 'supplies', 'connects'];
  const isConn = (n) => n && n.nodetype === 'code' && n.level === 'connection';
  const isReqFile = (n) => !!n && (n.nodetype === 'bizreq' || n.nodetype === 'techreq');
  const isReq = (n) => !!n && n.nodetype === 'req';
  /** Which edge kinds may join from → to (spec §2.4); the server validates too. */
  const RULES = {
    codenodes: (f, t) => t.nodetype === 'code' && !isConn(t) && !isConn(f) && !['usecase', 'actor', 'test'].includes(f.nodetype) && !REQ_TYPES.includes(f.nodetype),
    tests: (f, t) => t.nodetype === 'test' && f.nodetype !== 'test',
    reqs: (f, t) => REQ_TYPES.includes(t.nodetype) && f !== t,
    supplies: (f, t) => f.nodetype === 'code' && !isConn(f) && isConn(t),
    connects: (f, t) => isConn(f) && t.nodetype === 'code' && !isConn(t),
    drives: (f, t) => f.nodetype === 'actor' && t.nodetype === 'usecase',
    touches: (f, t) => f.nodetype === 'actor' && t.nodetype === 'code',
    uses: (f, t) => f.nodetype === 'usecase' && t.nodetype === 'code',
    contains: (f, t) => isReqFile(f) && isReq(t),
  };
  const VERB = { codenodes: 'owns', tests: 'is tested by', reqs: 'must meet', supplies: 'supplies', connects: 'connects to', drives: 'drives', touches: 'touches', uses: 'uses', contains: 'holds' };
  const LV = () => (window.IterGraph && IterGraph.LEVELS) || {};

  window.IterGraphEdit = {
    /** Called after each draw. ctx = the page's graph context (api, project, myRole, isAdmin, onData, openItem, openSettings). */
    attach(root, ctx) {
      S = { root, ctx, data: null, canEdit: ctx.myRole !== 'viewer' };
      const g = window.IterGraph && IterGraph.api;
      if (!g) return;
      S.data = g.model.G;
      if (S.data && S.data.iter4) S.canEdit = false; // an iter4-shaped server: nothing here would land
      renderBar();
      wireCy(g.cy);
      if (!keyWired) { keyWired = true; document.addEventListener('keydown', onKey); }
      // the tab element outlives each drawing: one listener of each, re-pointed at this S
      if (LISTEN.root) { LISTEN.root.removeEventListener('g-detail', LISTEN.detail); LISTEN.root.removeEventListener('g-select', LISTEN.sel); }
      LISTEN.root = root;
      root.addEventListener('g-detail', LISTEN.detail);
      root.addEventListener('g-select', LISTEN.sel);
      afterData();
      refreshBuild();
      loadConflicts();
    },
    get drawing() { return !!DRAW; },
    helpGroups() {
      if (!S || !S.canEdit) return [['Editing', [['—', 'Read only: viewers can look, not change']]]];
      return [
        ['Editing nodes', [['N', 'New node (attached to the selected node)'], ['E / double-click', 'Configure the selected node or edge'], ['T', 'Run the selected node\'s tests'], ['Del', 'Delete the selected node (asks why)'], ['Right-click / long-press', 'Everything you can do to a node or edge']]],
        ['Editing edges', [['C', 'Connect: draw an edge from the selected node'], ['Drag an edge end', 'Select an edge, drag a round handle onto another node'], ['Mod+C', 'Copy the selected edge'], ['Mod+V', 'Paste it onto the selected node'], ['Del', 'Remove the selected edge (asks why)']]],
      ];
    },
  };

  const P = () => '/api/projects/' + enc(S.ctx.project);
  const api = (path, opts) => S.ctx.api(P() + path, opts);
  const G = () => window.IterGraph && IterGraph.api;
  const model = () => G().model;
  const node = (id) => model().nodesById.get(id);
  const nameOf = (id) => (node(id) || {}).name || id;
  const typeLabel = (n) => ((LV()[n.level] || {}).label || n.level);
  const toast = (m, o) => K() && K().toast(m, o);
  const errMsg = (e) => {
    const b = e && e.body;
    if (b && b.refused === 'glob') return `that link comes from the glob "${b.entry || '?'}" in ${b.owner ? (nameOf(b.owner) || b.owner) : 'the owning node'}'s file, so it cannot be removed on its own — open Configure on ${b.owner ? nameOf(b.owner) : 'that node'} and narrow the glob`;
    return (b && b.error) || (e && e.message) || String(e);
  };

  // ------------------------------------------------------------------ data refresh + sync state
  async function refresh(focusId) {
    let raw;
    try { raw = await S.ctx.api(P() + '/graph'); } catch (e) { toast('Could not reload the graph: ' + errMsg(e), { kind: 'err' }); return; }
    if (S.ctx.onData) S.ctx.onData(raw);
    IterGraph.update(raw);
    S.data = model().G;
    afterData();
    loadConflicts();
    if (focusId) setTimeout(() => G() && G().focusNode(focusId, { flash: true }), 60);
  }
  function counts() {
    const c = { pending: 0, del: 0, designed: 0 };
    (S.data ? S.data.nodes : []).forEach((n) => { if (n.nodetype === 'req') return; if (n.file_state === 'pending_write') c.pending++; else if (n.file_state === 'pending_delete') c.del++; else if (n.file_state === 'designed') c.designed++; });
    return c;
  }
  function afterData() {
    renderSync(); renderDesignBar();
    clearTimeout(pollTimer);
    const c = counts();
    if (c.pending + c.del > 0) pollTimer = setTimeout(poll, 5000);
  }
  async function poll() {
    if (!S || !G()) return;
    if (!S.root.offsetParent) { pollTimer = setTimeout(poll, 5000); return; } // the tab is hidden: look again later
    const before = JSON.stringify(S.data.nodes.map((n) => [n.id, n.file_state, n.node_version]));
    let raw; try { raw = await S.ctx.api(P() + '/graph'); } catch (e) { pollTimer = setTimeout(poll, 8000); return; }
    const fresh = IterGraph.normalize(raw);
    const after = JSON.stringify(fresh.nodes.map((n) => [n.id, n.file_state, n.node_version]));
    if (before !== after) { if (S.ctx.onData) S.ctx.onData(raw); IterGraph.update(raw); S.data = model().G; }
    afterData();
  }
  function renderSync() {
    const el = S.root.querySelector('#g-sync'); if (!el) return;
    const c = counts(); const n = c.pending + c.del;
    el.className = 'g-sync ' + (n ? 'pending' : c.designed ? 'designed' : 'synced');
    el.innerHTML = n ? `<span class="g-spin"></span>${n} pending sync` : c.designed ? `✎ ${c.designed} designed` : '✓ synced';
    el.title = n ? `${c.pending} node(s) changed here and ${c.del} deleted here, waiting for an engine to write the files. Checking every 5 seconds. Click for the list.`
      : c.designed ? 'Every node exists only in the designer: Build creates the repository.' : 'Every node matches its file in the repository.';
  }
  function showPendingList() {
    const list = S.data.nodes.filter((n) => n.file_state !== 'synced' && n.nodetype !== 'req');
    const FS = IterGraph.FILE_STATES;
    const rows = list.slice(0, 300).map((n) => `<tr><td><a href="#" data-focus="${esc(n.id)}">${esc(n.name)}</a></td><td>${esc(typeLabel(n))}</td><td><span class="g-fs g-fs-${esc(n.file_state)}">${esc(FS[n.file_state].label)}</span></td><td class="g-mono">${esc(n.path)}</td></tr>`).join('');
    K().modal({ title: 'File sync — ' + S.ctx.project, wide: true, confirmDiscard: false,
      body: `<p class="kit-text kit-dim">An edit in the graph is saved at once; an engine serving the project writes the file and commits it on its next heartbeat. A path inside a folder a running work item has locked waits for that item.</p>
        ${list.length ? `<div class="g-tablewrap"><table class="g-table"><tr><th>node</th><th>type</th><th>state</th><th>file</th></tr>${rows}</table></div>` : '<p>Everything is in sync.</p>'}`,
      buttons: [{ label: 'Close', value: null, primary: true }],
      onOpen: (d) => d.querySelectorAll('[data-focus]').forEach((a) => { a.onclick = (e) => { e.preventDefault(); d.finish(null); G().focusNode(a.dataset.focus, { flash: true }); }; }) });
  }

  // ------------------------------------------------------------------ toolbar
  function renderBar(selOnly) {
    const bar = S.root.querySelector('#g-editbar'); if (!bar) return;
    if (!selOnly) {
      if (!S.canEdit) {
        bar.innerHTML = `<span id="g-buildstate" class="g-buildstate hidden"></span><button type="button" id="g-conflicts" class="g-conflictsbtn hidden"></button><span class="g-sync" id="g-sync"></span><span class="g-ro">${S.data && S.data.iter4 ? 'read only — this server returns the iter4 graph shape' : 'read only (viewer)'}</span>`;
        const s = bar.querySelector('#g-sync'); if (s) s.onclick = showPendingList;
        bar.querySelector('#g-conflicts').onclick = showConflicts;
        renderConflictBadge(); renderBuildState();
        return;
      }
      bar.innerHTML = '<span id="g-buildstate" class="g-buildstate hidden"></span><button type="button" id="g-conflicts" class="g-conflictsbtn hidden"></button><button type="button" class="g-sync" id="g-sync"></button>'
        + '<button type="button" class="g-btn g-primary" data-ed="new" title="Add a node: a context, container, component, connection, test, requirement, use case or actor (N)">+ Node</button>'
        + '<button type="button" class="g-btn" data-ed="connect" title="Draw an edge from the selected node to another (C)">Connect</button>'
        + '<button type="button" class="g-btn" data-ed="paste" title="Paste the copied edge onto the selected node (⌘V)">Paste edge</button>'
        + '<button type="button" class="g-btn" data-ed="tests" title="Queue a test run for the selected node (T)">Run tests</button>';
      bar.querySelector('#g-sync').onclick = showPendingList;
      bar.querySelector('#g-conflicts').onclick = showConflicts;
      bar.querySelectorAll('[data-ed]').forEach((b) => { b.onclick = () => toolbar(b.dataset.ed); });
    }
    const sel = G() && G().selected; const sn = sel && sel.kind === 'node' ? node(sel.id) : null;
    const set = (k, dis, tip) => { const b = bar.querySelector(`[data-ed="${k}"]`); if (b) { b.disabled = dis; if (tip) b.title = tip; } };
    set('connect', !sn, sn ? `Draw an edge from ${sn.name} (C)` : 'Select a node first, then draw an edge from it (C)');
    const clip = K().clip.project;
    set('paste', !clip || !sn, clip ? `Paste the copied "${clip.kind}" edge onto the selected node (⌘V)` : 'Copy an edge first (select it, ⌘C)');
    set('tests', !sn || !canTest(sn), sn ? `Queue a test run for ${sn.name} (T)` : 'Select a code or test node first (T)');
    renderSync(); renderConflictBadge(); renderBuildState();
  }
  function toolbar(k) {
    const sel = G().selected; const sn = sel && sel.kind === 'node' ? node(sel.id) : null;
    if (k === 'new') return newNode(sn);
    if (k === 'connect' && sn) return startDraw(sn);
    if (k === 'paste' && sn) return pasteEdge(sn);
    if (k === 'tests' && sn) return runTests(sn);
  }
  const canTest = (n) => n && (n.nodetype === 'code' || n.nodetype === 'test' || n.nodetype === 'project');

  // ------------------------------------------------------------------ designer banner + Build
  async function refreshBuild() {
    if (!S) return;
    const p = S.ctx.project;
    try { BUILD = await api('/build'); } catch (e) { BUILD = null; }
    if (!S || S.ctx.project !== p) return;
    renderDesignBar();
    clearTimeout(buildTimer);
    const st = BUILD && (BUILD.state || (BUILD.build && BUILD.build.state));
    if (st === 'requested' || st === 'running') buildTimer = setTimeout(watchBuild, 5000);
  }
  async function watchBuild() {
    const was = BUILD && (BUILD.state || (BUILD.build && BUILD.build.state));
    await refreshBuild();
    const now = BUILD && (BUILD.state || (BUILD.build && BUILD.build.state));
    if (was !== now && (now === 'done' || now === 'built')) { toast(`${S.ctx.project} is built: the repository exists and the files are committed.`, { kind: 'ok', ms: 6000 }); refresh(); }
  }
  function renderDesignBar() {
    const bar = S.root.querySelector('#g-designbar'); if (!bar) return;
    const b = BUILD ? (BUILD.build || BUILD) : null; const st = b && b.state;
    const c = counts();
    let html = '';
    if (st === 'requested' || st === 'running') {
      html = `<span class="g-db-icon"><span class="g-spin"></span></span><div class="g-db-txt"><b>Building on ${esc(b.engine || 'the engine')}</b><span>${st === 'running' ? 'The engine is creating the repository and writing the files.' : 'Waiting for the engine to pick the build up on its next heartbeat.'}${b.topdir ? ` Folder <code>${esc(b.topdir)}</code>.` : ''}</span></div>`;
    } else if (st === 'failed' || st === 'error') {
      html = `<span class="g-db-icon bad">!</span><div class="g-db-txt"><b>The build failed</b><span>${esc(b.error || b.message || 'The engine reported an error.')}</span></div>${S.canEdit ? '<button type="button" class="g-btn g-primary" data-build="1">Build again…</button>' : ''}`;
    } else if (c.designed) {
      html = `<span class="g-db-icon">✎</span><div class="g-db-txt"><b>Designed — not built yet</b><span>${c.designed} ${c.designed === 1 ? 'node exists' : 'nodes exist'} only here. Design freely; Build creates the repository on an engine, writes every file and commits it.</span></div>${S.canEdit ? '<button type="button" class="g-btn g-primary" data-build="1" title="Pick an engine and a folder; the engine creates the repository">Build…</button>' : ''}`;
    }
    bar.innerHTML = html; bar.classList.toggle('hidden', !html);
    bar.className = 'g-designbar' + (html ? '' : ' hidden') + (st === 'failed' || st === 'error' ? ' bad' : st === 'requested' || st === 'running' ? ' busy' : '');
    const btn = bar.querySelector('[data-build]'); if (btn) btn.onclick = () => openBuild();
    renderBuildState();
    if (G()) G().cy.resize();
  }
  const fmtWhen = (t) => { if (!t) return ''; const d = new Date(String(t).replace(' ', 'T')); return isNaN(d) ? String(t) : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }); };
  /** The toolbar pill for a built project: where its repository lives. */
  function renderBuildState() {
    const el = S && S.root.querySelector('#g-buildstate'); if (!el) return;
    const b = BUILD ? (BUILD.build || null) : null; const st = b && b.state;
    const built = st === 'done' || st === 'built';
    el.classList.toggle('hidden', !built);
    if (!built) { el.innerHTML = ''; const bs = S.root.querySelector('#g-detail .g-build-slot'); if (bs) bs.innerHTML = buildInfoHtml(); return; }
    el.innerHTML = `<span class="g-bs-dot"></span>built on <b>${esc(b.engine || '?')}</b>`;
    const bs = S.root.querySelector('#g-detail .g-build-slot'); if (bs) bs.innerHTML = buildInfoHtml();
    el.title = `Repository ${b.topdir || ''} on engine ${b.engine || '?'}${b.commit ? `, first commit ${String(b.commit).slice(0, 10)}` : ''}${b.done_at ? `, ${fmtWhen(b.done_at)}` : ''}. Every graph edit reaches it as a pending write.`;
  }
  /** The project node's Repository block (build record + file sync). */
  function buildInfoHtml() {
    const b = BUILD ? (BUILD.build || null) : null;
    const c = counts();
    if (!b || !b.state) {
      if (BUILD && BUILD.served) return `<h4>Repository</h4><p class="muted">Served by an engine; no build record (the repository existed before it was linked).</p>`;
      return `<h4>Repository</h4><p class="muted">${c.designed ? 'Not built yet: Build creates it on an engine.' : 'No repository yet.'}</p>`;
    }
    const st = { requested: 'build requested', running: 'building', done: 'built', built: 'built', failed: 'build failed', error: 'build failed' }[b.state] || b.state;
    const cls = b.state === 'done' || b.state === 'built' ? 'ok' : b.state === 'failed' || b.state === 'error' ? 'bad' : '';
    return `<h4>Repository</h4><div class="g-buildinfo"><dl class="g-facts"><dt>state</dt><dd><span class="g-pill ${cls}">${esc(st)}</span></dd><dt>engine</dt><dd>${esc(b.engine || '—')}</dd><dt>folder</dt><dd class="g-mono">${esc(b.topdir || '—')}</dd>${b.commit ? `<dt>commit</dt><dd class="g-mono">${esc(String(b.commit).slice(0, 12))}</dd>` : ''}<dt>requested</dt><dd>${esc(fmtWhen(b.at) || '—')}${b.by ? ` <span class="muted">by ${esc(b.by)}</span>` : ''}</dd>${b.done_at ? `<dt>finished</dt><dd>${esc(fmtWhen(b.done_at))}</dd>` : ''}${b.error ? `<dt>error</dt><dd class="g-bad">${esc(b.error)}</dd>` : ''}<dt>files</dt><dd>${BUILD.pending ? `${esc(BUILD.pending)} waiting for the engine` : 'all written'}</dd></dl></div>`;
  }

  // ------------------------------------------------------------------ sync conflicts (GET graph/conflicts)
  async function loadConflicts() {
    if (!S) return;
    const p = S.ctx.project;
    let rows = [];
    try { const r = await api('/graph/conflicts'); rows = (r && r.conflicts) || []; } catch (e) { rows = []; }
    if (!S || S.ctx.project !== p) return;
    CONFLICTS = rows;
    renderConflictBadge();
    const g = G(); const sel = g && g.selected;
    if (sel && sel.kind === 'node') { const box = S.root.querySelector('#g-detail'); if (box && !box.classList.contains('hidden')) fillConflicts(box, sel.id); }
  }
  function renderConflictBadge() {
    const el = S && S.root.querySelector('#g-conflicts'); if (!el) return;
    const n = CONFLICTS.length;
    el.classList.toggle('hidden', !n);
    el.innerHTML = n ? `⚠ ${n} conflict${n === 1 ? '' : 's'}` : '';
    el.title = n ? 'A node was changed here and its file was changed in the repository before the engine wrote it. The newer side won; the other version is kept. Click for the list.' : '';
  }
  const conflictText = (c) => `${c.winner === 'server' ? 'kept the graph\'s version' : c.winner === 'file' ? 'kept the file\'s version' : 'resolved'}`;
  function conflictLoser(c) {
    const l = c.loser || {};
    const text = typeof l.text === 'string' ? l.text : l.doc && typeof l.doc.body === 'string' ? l.doc.body : (l.body != null ? `${l.name || ''}\n\n${l.desc || ''}\n\n${l.body || ''}` : JSON.stringify(l, null, 2));
    return `<details class="g-loser"><summary>The version that lost (${c.winner === 'server' ? 'the file' : 'the graph\'s'})</summary><pre>${esc(String(text).slice(0, 6000))}</pre></details>`;
  }
  function fillConflicts(box, id) {
    const slot = box.querySelector('.g-conflicts-slot'); if (!slot) return;
    const mine = CONFLICTS.filter((c) => c.id === id);
    slot.innerHTML = mine.length ? `<div class="g-conflicts"><div class="g-cf-head">⚠ ${mine.length} sync conflict${mine.length === 1 ? '' : 's'}</div>${mine.slice(0, 5).map((c) => `<div class="g-cf"><div><b>${esc(conflictText(c))}</b> <span class="muted">${esc(fmtWhen(c.at))}${c.engine ? ' · ' + esc(c.engine) : ''}</span></div><div class="muted">${esc(c.why || '')}</div>${conflictLoser(c)}</div>`).join('')}</div>` : '';
  }
  function showConflicts() {
    const rows = CONFLICTS.slice(0, 300).map((c) => { const n = node(c.id); return `<tr><td>${n ? `<a href="#" data-focus="${esc(c.id)}">${esc(n.name)}</a>` : `<span class="muted">${esc(String(c.id).slice(-12))} (gone)</span>`}</td><td>${esc(conflictText(c))}</td><td>${esc(c.why || '')}</td><td class="g-nowrap">${esc(fmtWhen(c.at))}</td><td class="g-mono">${esc(c.path || '')}</td></tr>`; }).join('');
    K().modal({ title: 'Sync conflicts — ' + S.ctx.project, wide: true, confirmDiscard: false,
      body: `<p class="kit-text kit-dim">Each row is a node that changed here while its file changed in the repository. The newer edit won (a tie goes to the graph); the other version is kept on the node's detail pane.</p>
        ${CONFLICTS.length ? `<div class="g-tablewrap"><table class="g-table"><tr><th>node</th><th>outcome</th><th>why</th><th>when</th><th>file</th></tr>${rows}</table></div>` : '<p>No conflicts.</p>'}`,
      buttons: [{ label: 'Close', value: null, primary: true }],
      onOpen: (d) => d.querySelectorAll('[data-focus]').forEach((a) => { a.onclick = (e) => { e.preventDefault(); d.finish(null); G().focusNode(a.dataset.focus, { flash: true }); }; }) });
  }

  // ------------------------------------------------------------------ test history (GET testlogs?node=)
  async function fillTestLogs(box, n) {
    const slot = box.querySelector('.g-testlogs-slot'); if (!slot) return;
    slot.innerHTML = '<h4>History</h4><p class="muted">loading…</p>';
    let logs = [];
    try { const r = await api('/testlogs?node=' + enc(n.id) + '&limit=10'); logs = (r && r.logs) || []; } catch (e) { slot.innerHTML = `<h4>History</h4><p class="muted">Could not load the test log: ${esc(errMsg(e))}</p>`; return; }
    if (!slot.isConnected) return;
    if (!logs.length) { slot.innerHTML = '<h4>History</h4><p class="muted">No runs recorded yet. Run tests queues one; the engine posts each result here.</p>'; return; }
    const cnt = (b) => (b && typeof b === 'object' && b.total ? `${b.pass ?? 0}/${b.total}` : '');
    const row = (l) => {
      const ok = l.overall_success != null ? l.overall_success : l.outcome === 'pass';
      const outcome = l.outcome || (ok ? 'pass' : 'fail');
      const buckets = ['normal', 'longtail', 'failure'].map((k) => cnt(l[k]) ? `<span title="${k}">${k[0].toUpperCase() + k.slice(1)} ${cnt(l[k])}</span>` : '').filter(Boolean).join('');
      const bad = (Array.isArray(l.details) ? l.details : []).filter((d) => d && d.pass === false);
      const wid = l.workitem && l.workitem.id;
      return `<li class="g-tlog ${ok ? 'ok' : 'bad'}"><div class="g-tlog-top"><span class="g-tlog-icon">${ok ? '✓' : '✗'}</span><b>${esc(outcome)}</b><span class="muted">${esc(fmtWhen(l.at))}</span>${l.exit_code != null ? `<span class="muted">exit ${esc(l.exit_code)}</span>` : ''}</div>
        <div class="g-tlog-sub">${buckets}${l.engine ? `<span class="muted">on ${esc(l.engine)}</span>` : ''}${l.commit ? `<span class="g-mono muted">${esc(String(l.commit).slice(0, 8))}</span>` : ''}${wid ? `<a data-open="${esc(wid)}" title="the work item this failure filed">item …${esc(String(wid).slice(-12))}</a>` : ''}</div>
        ${bad.length ? `<ul class="g-tlog-bad">${bad.slice(0, 6).map((d) => `<li>✗ ${esc(d.name || '')}${d.msg ? ` <span class="muted">— ${esc(d.msg)}</span>` : ''}</li>`).join('')}${bad.length > 6 ? `<li class="muted">… ${bad.length - 6} more</li>` : ''}</ul>` : ''}</li>`;
    };
    slot.innerHTML = `<h4>History <span class="muted">(last ${logs.length})</span></h4><ul class="g-testlogs">${logs.map(row).join('')}</ul>`;
    slot.querySelectorAll('[data-open]').forEach((a) => { a.onclick = () => S.ctx.openItem && S.ctx.openItem(a.dataset.open); });
  }
  /** Engines to build on: `name` is the engine's stable id (what the server takes), `label` its display name. */
  async function engineChoices() {
    const label = new Map(); const live = new Map();
    try {
      const sg = await S.ctx.api('/api/settings/graph');
      (sg.nodes || []).filter((n) => n.type === 'iter_engine' && !n.placeholder && !n.deactivated).forEach((n) => { label.set(n.key || String(n.id).replace(/^iter_engine:/, ''), n.name); });
    } catch (e) { /* settings graph not there yet */ }
    try {
      const es = await S.ctx.api('/api/engines');
      (es || []).forEach((e) => { const id = e.id || e.name; if (!label.has(id)) label.set(id, e.name || id); const t = Date.parse(e.last_seen || ''); live.set(id, t && Date.now() - t < 3 * (e.ticksec || 5) * 1000 + 5000); });
    } catch (e) { /* none */ }
    return [...label.keys()].filter(Boolean).sort().map((n) => ({ name: n, label: label.get(n) || n, online: !!live.get(n) }));
  }
  async function openBuild() {
    const proj = S.ctx.project;
    const slug = String(proj).toLowerCase().replace(/[^a-z0-9_-]+/g, '_').replace(/^_+|_+$/g, '') || 'project';
    const engines = await engineChoices();
    const pnode = S.data.nodes.find((n) => n.nodetype === 'project');
    const gitrepo = pnode && pnode.front && pnode.front.gitrepo;
    const opts = engines.map((e) => `<option value="${esc(e.name)}">${esc(e.label)}${e.online ? ' — online' : ' — offline'}</option>`).join('');
    const firstOnline = (engines.find((e) => e.online) || engines[0] || {}).name || '';
    const body = `<p class="kit-text">The engine creates the folder, runs <code>git init</code>${gitrepo ? ` (with remote <code>${esc(gitrepo)}</code>)` : ''}, writes every designed node as a <code>*.iter.md</code> file and makes the first commit, <code>iter: build from design</code>. Later edits here reach the repository the same way, as pending writes.</p>
      <label class="kit-field"><span>Engine</span>${engines.length ? `<select id="b-eng">${opts}</select>` : `<input id="b-eng" type="text" placeholder="engine name" autocomplete="off">`}
        <small>${engines.length ? 'The machine that will hold the repository. It must be running: start one with <code>iter_engine --data-url URL --env-file PATH</code>.' : 'No engine is registered yet. On the machine that should hold the code run <code>iter_engine --data-url ' + esc(location.origin) + ' --env-file ~/.iter5/.env</code> once; it registers itself and appears here.'}</small></label>
      <label class="kit-field"><span>Folder on that machine (topdir)</span><input id="b-top" type="text" value="~/dev/${esc(slug)}" autocomplete="off" spellcheck="false"><small>Created if missing; an existing git repository there is reused.</small></label>
      <label class="kit-check"><input type="checkbox" id="b-plan" checked><span>Queue the plan agent<small>A <b>plan</b> work item at priority 5 on the project node: "Build the project from its design".</small></span></label>
      <label class="kit-field" id="b-notewrap"><span>Note for the plan agent (optional)</span><textarea id="b-note" rows="3" placeholder="Anything the plan should know: what to build first, what to leave out…"></textarea></label>`;
    return K().modal({ title: `Build ${proj}`, sub: 'Turn the design into a repository', body, wide: true,
      buttons: [{ label: 'Cancel', value: null }, { label: 'Build', value: 'build', primary: true }],
      onOpen: (d) => {
        const e = d.querySelector('#b-eng'); if (e && e.tagName === 'SELECT' && firstOnline) e.value = firstOnline;
        const pl = d.querySelector('#b-plan'); pl.onchange = () => { d.querySelector('#b-notewrap').classList.toggle('hidden', !pl.checked); };
      },
      onButton: async (v, d) => {
        let engine = d.querySelector('#b-eng').value.trim(); const topdir = d.querySelector('#b-top').value.trim();
        if (!engine) throw new Error('Pick the engine that will hold the repository.');
        // the server refuses an engine it has no record of (404): say so here instead of sending it
        const known = engines.length ? engines : await engineChoices();
        const hit = known.find((e) => e.name === engine || e.label === engine); if (hit) engine = hit.name;
        if (!hit) {
          d.querySelector('#b-eng').classList.add('kit-bad');
          throw new Error(known.length ? `There is no engine called "${engine}". Registered: ${known.map((e) => e.label).join(', ')}.`
            : `There is no engine called "${engine}" yet: start it once on that machine (the command above), then Build again.`);
        }
        if (!topdir) throw new Error('Say where the repository goes on that machine.');
        const queue_plan = d.querySelector('#b-plan').checked; const note = d.querySelector('#b-note').value.trim();
        const r = await api('/build', { method: 'POST', body: JSON.stringify(Object.assign({ engine, topdir, queue_plan }, queue_plan && note ? { plan_note: note } : {})) });
        BUILD = r && (r.state || r.build) ? r : { state: 'requested', engine, topdir };
        if (!BUILD.engine) BUILD.engine = engine;
        renderDesignBar();
        toast(`Build requested on ${engine}. The engine picks it up on its next heartbeat.`, { kind: 'ok' });
        clearTimeout(buildTimer); buildTimer = setTimeout(watchBuild, 4000);
        refresh();
        return true;
      } });
  }

  // ------------------------------------------------------------------ cytoscape wiring
  function wireCy(cy) {
    const host = S.root.querySelector('#g-cywrap');
    const at = (e) => e.renderedPosition || (e.target.isEdge && e.target.isEdge() ? e.target.renderedMidpoint() : e.target.renderedPosition());
    const isHelper = (el) => el.hasClass('kit-ep') || el.hasClass('anchor') || el.id().startsWith('__');
    cy.on('cxttap', 'node', (e) => { if (!isHelper(e.target)) { const n = node(e.target.id()); if (n) { G().select({ kind: 'node', id: n.id }); nodeMenu(n, at(e)); } } });
    cy.on('taphold', 'node', (e) => { if (!isHelper(e.target)) { const n = node(e.target.id()); if (n) nodeMenu(n, at(e)); } });
    cy.on('cxttap', 'edge', (e) => { if (!isHelper(e.target)) { G().select({ kind: 'edge', id: e.target.id() }); edgeMenu(e.target, at(e)); } });
    cy.on('taphold', 'edge', (e) => { if (!isHelper(e.target)) edgeMenu(e.target, at(e)); });
    cy.on('cxttap', (e) => { if (e.target === cy) canvasMenu(at(e)); });
    cy.on('tap', 'node', (e) => {
      if (isHelper(e.target)) return;
      if (DRAW) { finishDraw(e.target.id()); return; }
      const oe = e.originalEvent || {};
      if (oe.metaKey || oe.ctrlKey) { const n = node(e.target.id()); if (n) nodeMenu(n, at(e)); }
    });
    cy.on('dbltap', 'node', (e) => { if (!isHelper(e.target)) { const n = node(e.target.id()); if (n) configureNode(n); } });
    cy.on('dbltap', 'edge', (e) => { if (!isHelper(e.target)) { const r = linkOf(e.target); if (r) configureEdge(r); } });
    cy.on('tap', (e) => { if (e.target === cy && DRAW) cancelDraw(); });
    if (host) host.addEventListener('contextmenu', (ev) => ev.preventDefault());
    K().endpointHandles(cy, {
      canEdit: (edge) => S.canEdit && !!linkOf(edge),
      accept: (edge, end, n) => {
        const r = linkOf(edge); const nn = node(n.id()); if (!r || !nn) return 'Drop it on a node.';
        if (r.kind === 'contains') {
          if (end === 'target') return 'Drag the file end of this line: a requirement moves to another file, the requirement itself stays.';
          return isReqFile(nn) ? true : `Drop it on a bizreq or techreq file: a ${typeLabel(nn).toLowerCase()} cannot hold a requirement.`;
        }
        const f = end === 'source' ? nn : node(r.from); const t = end === 'target' ? nn : node(r.to);
        if (f.id === t.id) return 'An edge cannot join a node to itself.';
        return RULES[r.kind] && RULES[r.kind](f, t) ? true : `A "${r.kind}" edge cannot ${end === 'source' ? 'start' : 'end'} at a ${typeLabel(nn).toLowerCase()}.`;
      },
      onDrop: (edge, end, n) => moveEdge(linkOf(edge), end, n.id()),
    });
  }
  /** The project-graph link a drawn edge stands for, or null (use-case steps are not links). */
  function linkOf(edge) {
    const r = G() && G().edgeRecord(edge.id());
    return r && r.type === 'link' ? r.link : null;
  }

  // ------------------------------------------------------------------ menus
  function nodeMenu(n, pos) {
    const host = S.root.querySelector('#g-cywrap');
    if (isReq(n)) {
      const items = S.canEdit ? [
        { key: 'edit', label: 'Edit requirement…', kbd: 'E', hint: 'key, title, status and text' },
        { key: 'add', label: 'Add requirement after it…', kbd: 'N' },
        { key: 'move', label: 'Move to another file…', hint: 'keeps its id' },
        { key: 'file', label: 'Open its file' },
        { key: 'hide', label: 'Hide from the drawing' },
        { sep: true },
        { key: 'delete', label: 'Delete requirement…', kbd: 'Del', danger: true },
      ] : [{ key: 'edit', label: 'View…', kbd: 'E' }, { key: 'file', label: 'Open its file' }, { key: 'hide', label: 'Hide from the drawing' }];
      K().menu(host, pos, `Requirement · ${n.key || n.title}`, items, (k) => reqAction(k, n));
      return;
    }
    const clip = K().clip.project;
    const pin = { key: 'pin', label: G().isPinned(n.id) ? 'Unpin' : 'Pin in place', kbd: 'P', hint: 'it stays where it is through drags and layouts' };
    const items = S.canEdit ? [
      pin,
      { key: 'configure', label: 'Configure…', kbd: 'E', hint: 'every field of this node, editable' },
      { key: 'new', label: 'Add node here…', kbd: 'N', hint: 'a new node linked to this one' },
      { key: 'connect', label: 'Connect from here', kbd: 'C', hint: 'draw an edge to another node' },
      { key: 'paste', label: clip ? `Paste "${clip.kind}" edge here` : 'Paste edge here', kbd: K().MOD + 'V', disabled: !clip },
      { key: 'tests', label: 'Run tests', kbd: 'T', disabled: !canTest(n), hint: 'queue a test work item for this node' },
      { key: 'move', label: 'Move file…', hint: 'a new path for its *.iter.md file' },
      { key: 'hide', label: 'Hide from the drawing' },
      { sep: true },
      { key: 'delete', label: 'Delete node…', kbd: 'Del', danger: true, disabled: n.nodetype === 'project' },
    ] : [pin, { key: 'configure', label: 'View all fields…', kbd: 'E' }, { key: 'hide', label: 'Hide from the drawing' }];
    K().menu(host, pos, `${typeLabel(n)} · ${n.name}`, items, (k) => nodeAction(k, n));
  }
  function nodeAction(k, n) {
    if (k === 'pin') { G().togglePin(n.id); return; }
    if (isReq(n)) { reqAction({ configure: 'edit', new: 'add' }[k] || k, n); return; }
    ({ configure: () => configureNode(n), new: () => newNode(n), connect: () => startDraw(n), paste: () => pasteEdge(n), tests: () => runTests(n),
      move: () => moveFile(n), hide: () => G().hide(n.id), delete: () => deleteNode(n) })[k]();
  }
  function edgeMenu(edge, pos) {
    const r = linkOf(edge); if (!r) return;
    const host = S.root.querySelector('#g-cywrap');
    if (r.kind === 'contains') {
      const q = node(r.to); if (!q) return;
      K().menu(host, pos, `${nameOf(r.from)} holds ${q.key || q.title}`, S.canEdit
        ? [{ key: 'move', label: 'Move requirement to another file…', hint: 'or drag the file end of the line' }, { key: 'edit', label: 'Edit requirement…' }, { sep: true }, { key: 'delete', label: 'Delete requirement…', danger: true }]
        : [{ key: 'edit', label: 'View requirement…' }], (k) => reqAction(k, q));
      return;
    }
    const items = S.canEdit ? [
      { key: 'configure', label: 'Configure…', kbd: 'E' }, { key: 'copy', label: 'Copy edge', kbd: K().MOD + 'C', hint: 'then select a node and paste' },
      { sep: true }, { key: 'remove', label: 'Remove edge…', kbd: 'Del', danger: true },
    ] : [{ key: 'configure', label: 'View…' }, { key: 'copy', label: 'Copy edge' }];
    K().menu(host, pos, `${r.kind} · ${nameOf(r.from)} → ${nameOf(r.to)}`, items, (k) => ({ configure: () => configureEdge(r), copy: () => copyEdge(r), remove: () => removeEdge(r) })[k]());
  }
  function canvasMenu(pos) {
    if (!S.canEdit) return;
    const host = S.root.querySelector('#g-cywrap');
    K().menu(host, pos, 'Project graph', [{ key: 'new', label: 'New node…', kbd: 'N' }, { key: 'pending', label: 'File sync status…' }, { key: 'help', label: 'Shortcuts…', kbd: '?' }],
      (k) => ({ new: () => newNode(null), pending: showPendingList, help: () => G().openHelp() })[k]());
  }

  // ------------------------------------------------------------------ the detail pane's buttons
  function decorate(d) {
    if (!d || !S) return;
    if (d.kind === 'node' && d.node) {
      fillConflicts(d.box, d.node.id);
      const bs = d.box.querySelector('.g-build-slot'); if (bs) bs.innerHTML = buildInfoHtml();
      if (d.node.nodetype === 'test') fillTestLogs(d.box, d.node);
    }
    decorateReqs(d);
    const slot = d.box.querySelector('.g-actions-slot'); if (!slot) return;
    const btn = (k, t, tip, cls) => `<button type="button" class="g-btn ${cls || ''}" data-act="${k}" title="${esc(tip || '')}">${esc(t)}</button>`;
    if (d.kind === 'node' && d.node && isReq(d.node)) {
      const q = d.node;
      slot.innerHTML = `<div class="actions">${S.canEdit ? btn('edit', 'Edit…', 'key, title, status and text (E / double-click)', 'g-primary') + btn('add', '+ Requirement after', 'a new requirement in the same file, after this one (N)') + btn('move', 'Move to…', 'another bizreq / techreq file (keeps its id)') + btn('delete', 'Delete…', 'asks why', 'g-danger')
        : btn('edit', 'View…', '')}</div>`;
      slot.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => reqAction(b.dataset.act, q); });
    } else if (d.kind === 'edge' && d.edge && d.edge.type === 'link' && d.edge.kind === 'contains') {
      const q = node(d.edge.link.to);
      if (q) {
        slot.innerHTML = `<div class="actions">${S.canEdit ? btn('move', 'Move to…', 'another requirements file', 'g-primary') + btn('edit', 'Edit requirement…', '') : btn('edit', 'View requirement…', '')}</div>`;
        slot.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => reqAction(b.dataset.act, q); });
      }
    } else if (d.kind === 'node' && d.node) {
      const n = d.node;
      slot.innerHTML = `<div class="actions">${S.canEdit ? btn('configure', 'Configure…', 'every field (E / double-click)', 'g-primary') + btn('new', '+ Node here', 'a new node linked to this one (N)') + btn('connect', 'Connect', 'draw an edge from here (C)')
        + (canTest(n) ? btn('tests', 'Run tests', 'queue a test work item (T)') : '') + (n.nodetype !== 'project' ? btn('delete', 'Delete…', 'delete this node and its file (asks why)', 'g-danger') : '')
        : btn('configure', 'View all fields…', '')}</div>`;
      slot.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => nodeAction(b.dataset.act, n); });
    } else if (d.kind === 'edge' && d.edge && d.edge.type === 'link') {
      const r = d.edge.link;
      slot.innerHTML = `<div class="actions">${btn('configure', S.canEdit ? 'Configure…' : 'View…', '', 'g-primary')}${btn('copy', 'Copy edge', 'then select a node and paste (⌘C / ⌘V)')}${S.canEdit ? btn('remove', 'Remove…', 'asks why', 'g-danger') : ''}</div>`;
      slot.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => ({ configure: () => configureEdge(r), copy: () => copyEdge(r), remove: () => removeEdge(r) })[b.dataset.act](); });
    }
  }

  // ------------------------------------------------------------------ requirements (spec §2.8): one ## section of a bizreq/techreq file each
  const reqsOf = (fileId) => { const f = node(fileId); return (f && f.reqs) || []; };
  const reqName = (q) => (q.key ? `${q.key} — ${q.title}` : q.title || 'the requirement');
  const REQ_STATUS = ['draft', 'agreed', 'done'];
  /** Buttons on a requirements file's table, a code node's files block and a req node's status pill. */
  function decorateReqs(d) {
    if (!d || d.kind !== 'node' || !d.node) return;
    const box = d.box; const n = d.node;
    const btn = (attrs, t, tip, cls) => `<button type="button" class="g-btn ${cls || ''}" ${attrs} title="${esc(tip || '')}">${esc(t)}</button>`;
    const tbl = box.querySelector('.g-reqs');
    if (tbl && isReqFile(n)) {
      const tools = tbl.querySelector('.g-reqs-tools');
      if (tools && S.canEdit) {
        tools.innerHTML = btn('data-radd="1"', '+ Add', 'Add a requirement to this file (a new ## section)', 'g-primary');
        tools.querySelector('[data-radd]').onclick = () => addReq({ file: n.id });
      }
      tbl.querySelectorAll('[data-req-acts]').forEach((el) => {
        const q = node(el.dataset.reqActs); if (!q) return;
        el.innerHTML = (S.canEdit ? btn('data-ract="edit"', 'Edit…', 'key, title, status and text', 'g-primary') + btn('data-ract="add"', '+ After', 'a new requirement right after this one') + btn('data-ract="move"', 'Move to…', 'another bizreq / techreq file (keeps its id)') + btn('data-ract="delete"', 'Delete…', 'asks why', 'g-danger') : '')
          + btn('data-ract="focus"', 'Show in graph', 'draw the requirement nodes and select this one');
        el.querySelectorAll('[data-ract]').forEach((b) => { b.onclick = (ev) => { ev.stopPropagation(); reqAction(b.dataset.ract, q); }; });
      });
    }
    if (S.canEdit) {
      box.querySelectorAll('.g-rst[data-rstatus]').forEach((pill) => {
        const q = node(pill.dataset.rstatus); if (!q || q.derived) return;
        pill.classList.add('edit'); pill.setAttribute('role', 'button'); pill.tabIndex = 0; pill.title = `Status: ${q.status}. Click to change it.`;
        const open = (ev) => { ev.stopPropagation(); ev.preventDefault(); statusMenu(q, pill); };
        pill.onclick = open; pill.onkeydown = (ev) => { if (ev.key === 'Enter' || ev.key === ' ') open(ev); };
      });
      box.querySelectorAll('.g-rfile-tools[data-rtools]').forEach((el) => {
        const type = el.dataset.rtools; const fid = el.dataset.file;
        el.innerHTML = btn(`data-rfadd="${type}"`, '+ Add', fid ? `Add a requirement to ${nameOf(fid)}` : `Create ${n.name}'s ${type} file with its first requirement`);
        el.querySelector('button').onclick = () => (fid ? addReq({ file: fid }) : addReq({ node: n.id, type }));
      });
    }
  }
  function reqAction(k, q) {
    if (!q) return;
    ({ edit: () => editReq(q), configure: () => editReq(q), add: () => addReq({ file: q.file, after: q.id }), new: () => addReq({ file: q.file, after: q.id }), move: () => moveReq(q), delete: () => deleteReq(q), hide: () => G().hide(q.id),
      file: () => G().focusNode(q.file, { flash: true }),
      focus: () => { G().showType('req'); G().focusNode(q.id, { flash: true }); } })[k]();
  }
  /** After a requirement change: reload, then keep the file's table (or the req pane) on screen with the row open. */
  async function afterReq(fileId, reqId, msg) {
    if (msg) toast(msg, { kind: 'ok' });
    await refresh();
    const g = G(); if (!g) return;
    const sel = g.selected;
    if (reqId && sel && sel.kind === 'node' && sel.id === fileId) g.expandReq(reqId);
  }
  function nextKey(fileId) {
    const keys = reqsOf(fileId).map((q) => q.key).filter(Boolean);
    let best = null;
    keys.forEach((k) => { const m = k.match(/^(.*?)(\d+)$/); if (m && (!best || +m[2] > best.n || (+m[2] === best.n && m[2].length > best.w))) best = { p: m[1], n: +m[2], w: m[2].length }; });
    return best ? best.p + String(best.n + 1).padStart(best.w, '0') : '';
  }
  const checkText = (t) => { if (/^##(?!#)\s/m.test(t)) throw new Error('Text: a line starting with "## " would start a new requirement. Use ### or lower for sub-headings.'); };
  /** o = {file, after?} (a file that exists) or {node, type} (the code node's file is created by the server). */
  async function addReq(o) {
    if (!S.canEdit) return;
    const f = o.file ? node(o.file) : null; const owner = o.node ? node(o.node) : null;
    const type = f ? f.nodetype : o.type;
    const list = f ? reqsOf(f.id) : [];
    const where = list.length ? `<label class="kit-field"><span>Position</span><select id="rq-after"><option value="">at the end</option>${list.map((q) => `<option value="${esc(q.id)}" ${q.id === o.after ? 'selected' : ''}>after ${esc(reqName(q))}</option>`).join('')}</select></label>` : '';
    const body = `<div class="kit-row2"><label class="kit-field"><span>Key</span><input id="rq-key" type="text" value="${esc(f ? nextKey(f.id) : '')}" placeholder="e.g. PAY-BIZ-004" autocomplete="off" spellcheck="false"><small>optional; letters, digits, . _ -</small></label>
        <label class="kit-field"><span>Title</span><input id="rq-title" type="text" placeholder="the requirement in one line" autocomplete="off" autofocus></label></div>
      <label class="kit-field"><span>Requirement</span><textarea id="rq-text" rows="7" placeholder="What must be true, and why. Markdown; use ### or lower for sub-headings."></textarea></label>
      <div class="kit-row2"><label class="kit-field"><span>Status</span><select id="rq-status"><option>draft</option><option>agreed</option><option>done</option></select></label>${where}</div>
      <p class="kit-text kit-dim g-small">${f ? `Saved as a new <code>##</code> section of <code>${esc(f.path)}</code>` : `Creates <b>${esc(owner ? owner.name : '')}</b>'s ${type === 'techreq' ? 'technical' : 'business'} requirements file in its <code>reqs/</code> folder, lists it in the node's <code>children.reqs</code> and adds this as its first section`}; an engine writes the file within seconds.</p>`;
    let made = null;
    await K().modal({ title: f ? `Add a requirement — ${f.name}` : `Add a ${type === 'techreq' ? 'technical' : 'business'} requirement`, sub: f ? `${type === 'techreq' ? 'Technical' : 'Business'} requirements · ${list.length} so far` : (owner ? `for ${esc(owner.name)}` : ''),
      body, wide: true, cls: 'g-reqform', buttons: [{ label: 'Cancel', value: null }, { label: 'Add requirement', value: 'add', primary: true }],
      onButton: async (v, d) => {
        const key = d.querySelector('#rq-key').value.trim(); const title = d.querySelector('#rq-title').value.trim(); const text = d.querySelector('#rq-text').value.replace(/\s+$/, '');
        if (key && !/^[A-Za-z0-9_.\-]+$/.test(key)) { d.querySelector('#rq-key').classList.add('kit-bad'); throw new Error('Key: letters, digits, dot, underscore and dash only.'); }
        if (!title) { d.querySelector('#rq-title').classList.add('kit-bad'); throw new Error('A title is required.'); }
        checkText(text);
        const req = { title, text, status: d.querySelector('#rq-status').value };
        if (key) req.key = key;
        if (f) { req.file = f.id; const a = d.querySelector('#rq-after'); if (a && a.value) req.after = a.value; } else { req.node = o.node; req.type = type; }
        let r;
        try { r = await api('/graph/reqs', { method: 'POST', body: JSON.stringify(req) }); } catch (e) { throw new Error(errMsg(e)); }
        made = { req: (r && (r.req || r)) || {}, file: r && r.file };
        return true;
      } });
    if (!made) return;
    const mf = made.file; const fid = f ? f.id : (mf && (mf.id || (mf.doc && mf.doc.id) || (typeof mf === 'string' ? mf : ''))) || '';
    await afterReq(fid, made.req.id, `Added ${made.req.key || made.req.title || 'the requirement'}${f ? ` to ${f.name}` : ''}`);
  }
  async function editReq(q) {
    const f = node(q.file);
    const sibs = reqsOf(q.file); const at = sibs.findIndex((x) => x.id === q.id);
    return K().configure({
      title: `Requirement — ${q.key || q.title}`, sub: `${q.file_type === 'techreq' ? 'Technical' : 'Business'} requirement · one <code>##</code> section of ${esc(f ? f.name : 'its file')}`, readOnly: !S.canEdit || q.derived,
      meta: [['file', `<span class="g-mono">${esc(f ? f.path : '')}</span>`], ['section', at >= 0 ? `${at + 1} of ${sibs.length}` : '—'], ['id', `<span class="g-mono">${esc(q.id)}</span>`]],
      note: q.derived ? 'This section has no id yet: the server gives it one the next time the file is written; edit it then.' : 'Saved through the file: the section is rewritten in place, its id stays.',
      fields: [
        { key: 'key', value: q.key || '', type: 'text', help: 'optional: the part before " — " in the heading' },
        { key: 'title', value: q.title || '', type: 'text' },
        { key: 'status', value: REQ_STATUS.includes(q.status) ? q.status : 'draft', type: 'select', options: REQ_STATUS.map((x) => ({ value: x, label: x })), help: 'draft = proposed · agreed = everyone signed up · done = built and verified' },
        { key: 'text', value: q.text || '', type: 'markdown', rows: 10, help: 'markdown; use ### or lower for sub-headings' },
      ],
      onSave: async ({ changed }) => {
        if (!Object.keys(changed).length) return true;
        if ('title' in changed && !String(changed.title).trim()) throw new Error('title: a requirement needs a title');
        if ('key' in changed && changed.key && !/^[A-Za-z0-9_.\-]+$/.test(changed.key.trim())) throw new Error('key: letters, digits, dot, underscore and dash only');
        if ('text' in changed) checkText(changed.text);
        const patch = {};
        Object.keys(changed).forEach((k) => { patch[k] = typeof changed[k] === 'string' && k !== 'text' ? changed[k].trim() : changed[k]; });
        if (f && f.node_version != null) patch.expect_version = f.node_version;
        try { await api('/graph/reqs/' + enc(q.id), { method: 'PATCH', body: JSON.stringify(patch) }); }
        catch (e) { if (e.status === 409) throw new Error('The file changed since you opened this (another edit, or an engine). Close and open it again to see the new version.'); throw new Error(errMsg(e)); }
        afterReq(q.file, q.id, `Saved ${patch.key || q.key || patch.title || q.title}`);
        return true;
      },
    });
  }
  function statusMenu(q, pill) {
    const r = pill.getBoundingClientRect();
    K().menu(document.body, { x: r.left + window.scrollX - 6, y: r.bottom + window.scrollY - 2 }, `Status · ${q.key || q.title}`,
      REQ_STATUS.map((x) => ({ key: x, label: (x === q.status ? '✓ ' : '   ') + x, disabled: x === q.status, hint: { draft: 'proposed, not yet agreed', agreed: 'everyone signed up to it', done: 'built and verified' }[x] })),
      async (st) => {
        const f = node(q.file); const patch = { status: st }; if (f && f.node_version != null) patch.expect_version = f.node_version;
        try { await api('/graph/reqs/' + enc(q.id), { method: 'PATCH', body: JSON.stringify(patch) }); }
        catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
        afterReq(q.file, null, `${q.key || q.title}: ${st}`);
      });
  }
  async function moveReq(q, toFile) {
    if (!S.canEdit) return;
    let to = toFile;
    if (!to) {
      const owners = (fid) => S.data.edges.filter((e) => e.kind === 'reqs' && e.to === fid).map((e) => nameOf(e.from));
      const files = S.data.nodes.filter((n) => isReqFile(n) && n.id !== q.file)
        .sort((a, b) => (a.nodetype === q.file_type ? 0 : 1) - (b.nodetype === q.file_type ? 0 : 1) || a.name.localeCompare(b.name));
      if (!files.length) { toast('There is no other bizreq or techreq file in this project to move it to. Add a requirement to another node first (its file is created).', { kind: 'warn', ms: 6000 }); return; }
      to = await K().choose({ title: `Move ${q.key || 'requirement'}`, sub: esc(q.title || ''), okLabel: 'Move here',
        text: `The section leaves <b>${esc(nameOf(q.file))}</b> and is added at the end of the file you pick. It keeps its id, so links to it stay good.`,
        options: files.map((f) => { const o = owners(f.id); const c = (f.reqs || []).length; return { value: f.id, label: f.name, desc: `${f.nodetype === 'techreq' ? 'Technical' : 'Business'} · ${c} requirement${c === 1 ? '' : 's'}${o.length ? ' · required by ' + o.slice(0, 3).join(', ') : ''} · ${f.path}` }; }) });
      if (!to) return;
    }
    try { await api('/graph/reqs/' + enc(q.id) + '/move', { method: 'POST', body: JSON.stringify({ to_file: to }) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    await afterReq(q.file, null, `Moved ${q.key || q.title} to ${nameOf(to)}`);
  }
  async function deleteReq(q) {
    if (!S.canEdit) return;
    const reason = await K().ask({ title: `Delete ${q.key || 'requirement'}?`, text: `<b>${esc(reqName(q))}</b> — its <code>##</code> section comes out of <code>${esc((node(q.file) || {}).path || 'its file')}</code>; the rest of the file stays.`,
      label: 'Why?', help: 'Required — recorded with the change (and in the commit).', required: true, requiredMsg: 'Say why: it is recorded with the change.', multiline: true, rows: 2, okLabel: 'Delete requirement', danger: true });
    if (reason == null) return;
    try { await api('/graph/reqs/' + enc(q.id), { method: 'DELETE', body: JSON.stringify({ reason }) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    const sel = G().selected; if (sel && sel.id === q.id) G().select(null);
    await afterReq(q.file, null, `Deleted ${q.key || q.title}`);
  }

  // ------------------------------------------------------------------ create a node
  function attachOptions(parent, t) {
    if (!parent) return [];
    const fake = { id: '__new', nodetype: t.nodetype, level: t.level || t.nodetype, name: 'the new node' };
    const out = [];
    KIND_ORDER.forEach((k) => {
      if (RULES[k](parent, fake)) out.push({ kind: k, label: `${parent.name} ${VERB[k]} it` });
      else if (RULES[k](fake, parent)) out.push({ kind: k, label: `it ${VERB[k]} ${parent.name}` });
    });
    return out;
  }
  function defaultTypeFor(p) {
    if (!p) return 'context';
    if (p.nodetype === 'project') return 'context';
    if (p.nodetype === 'code') return { context: 'container', container: 'component', component: 'component', connection: 'container' }[p.level] || 'component';
    if (p.nodetype === 'usecase') return 'actor';
    if (p.nodetype === 'actor') return 'usecase';
    return 'test';
  }
  async function newNode(parent) {
    if (!S.canEdit) return;
    const nodes = S.data.nodes.filter((x) => !isReq(x)).sort((a, b) => IterGraph.LEVEL_ORDER.indexOf(a.level) - IterGraph.LEVEL_ORDER.indexOf(b.level) || a.name.localeCompare(b.name));
    const proj = nodes.find((n) => n.nodetype === 'project');
    const par = parent || proj || null;
    const groups = IterGraph.LEVEL_ORDER.map((lv) => { const list = nodes.filter((n) => n.level === lv); return list.length ? `<optgroup label="${esc((LV()[lv] || {}).label || lv)}">${list.map((n) => `<option value="${esc(n.id)}">${esc(n.name)}</option>`).join('')}</optgroup>` : ''; }).join('');
    const body = `<div class="g-types" role="radiogroup" aria-label="Type">${NEW_TYPES.map((t) => { const st = LV()[t.level || t.nodetype] || {}; return `<label class="g-type"><input type="radio" name="nt" value="${t.key}"><span class="g-chipsw ${esc(t.level || t.nodetype)}" style="--c:${st.color};--b:${st.border}"></span><span><b>${esc(t.label)}</b><small>${esc(t.hint)}</small></span></label>`; }).join('')}</div>
      <div class="kit-row2"><label class="kit-field"><span>Name</span><input id="nn-name" type="text" placeholder="what a person would call it" autocomplete="off" autofocus></label>
      <label class="kit-field"><span>Linked to</span><select id="nn-parent"><option value="">— nothing (stands alone) —</option>${groups}</select></label></div>
      <label class="kit-field" id="nn-kindwrap"><span>How</span><select id="nn-kind"></select><small id="nn-kindhelp"></small></label>
      <label class="kit-field"><span>Description</span><textarea id="nn-desc" rows="3" placeholder="~100 words: enough for an agent to decide whether to read the whole file"></textarea></label>
      <div id="nn-extra"></div>
      <details class="g-more"><summary>Body (markdown)</summary><textarea id="nn-body" rows="7" class="kit-mono" placeholder="# Title&#10;&#10;The requirement, the design, the notes…"></textarea></details>
      <p class="kit-text kit-dim g-small">The server names the file from the name and the folder rules (a component under its container's folder, requirements in <code>reqs/</code>, connections in <code>global/connections/</code>…). An engine writes it within seconds${counts().designed ? '; in a designed project it waits for Build' : ''}.</p>`;
    let created = null;
    await K().modal({ title: 'New node', sub: par ? `in ${S.ctx.project}` : '', body, wide: true,
      buttons: [{ label: 'Cancel', value: null }, { label: 'Create', value: 'create', primary: true }],
      onOpen: (d) => {
        const radios = d.querySelectorAll('input[name=nt]'); const ps = d.querySelector('#nn-parent'); const ks = d.querySelector('#nn-kind');
        const def = defaultTypeFor(par);
        radios.forEach((r) => { if (r.value === def) r.checked = true; });
        if (par) ps.value = par.id;
        const sync = () => {
          const t = NEW_TYPES.find((x) => x.key === (d.querySelector('input[name=nt]:checked') || {}).value) || NEW_TYPES[0];
          const p = node(ps.value);
          const opts = attachOptions(p, t);
          ks.innerHTML = opts.map((o) => `<option value="${o.kind}">${esc(o.label)} (${o.kind})</option>`).join('') + '<option value="">no link</option>';
          d.querySelector('#nn-kindwrap').classList.toggle('hidden', !p);
          d.querySelector('#nn-kindhelp').textContent = p && !opts.length ? `A ${t.label.toLowerCase()} cannot be linked to a ${typeLabel(p).toLowerCase()}; it is created on its own.` : '';
          const ex = d.querySelector('#nn-extra');
          ex.innerHTML = (t.nodetype === 'code' && t.level !== 'connection') ? `<label class="kit-field"><span>Owner</span><select id="nn-owner"><option value="">—</option><option>bespoke</option><option>oss</option><option>3rdparty</option></select><small>bespoke = written here · oss = open-source · 3rdparty = bought or hosted</small></label>`
            : (t.nodetype === 'bizreq' || t.nodetype === 'techreq') ? `<label class="kit-field"><span>Status</span><select id="nn-status"><option>draft</option><option>agreed</option><option>done</option></select></label>` : '';
        };
        radios.forEach((r) => { r.onchange = sync; }); ps.onchange = sync; sync();
      },
      onButton: async (v, d) => {
        const t = NEW_TYPES.find((x) => x.key === (d.querySelector('input[name=nt]:checked') || {}).value);
        if (!t) throw new Error('Pick a type.');
        const name = d.querySelector('#nn-name').value.trim();
        if (!name) { d.querySelector('#nn-name').classList.add('kit-bad'); throw new Error('A name is required.'); }
        const body = { nodetype: t.nodetype, name, desc: d.querySelector('#nn-desc').value.trim() };
        if (t.level) body.level = t.level;
        const b = d.querySelector('#nn-body').value; if (b.trim()) body.body = b;
        const pid = d.querySelector('#nn-parent').value; const kind = d.querySelector('#nn-kind').value;
        if (pid && kind) { body.attach_to = pid; body.attach_kind = kind; }
        const front = {};
        const ow = d.querySelector('#nn-owner'); if (ow && ow.value) front.owner = ow.value;
        const stt = d.querySelector('#nn-status'); if (stt) front.status = stt.value;
        if (Object.keys(front).length) body.front = front;
        const r = await api('/graph/nodes', { method: 'POST', body: JSON.stringify(body) });
        created = (r && (r.node || r)) || null;
        return true;
      } });
    if (created) {
      const lv = created.nodetype === 'code' ? created.level || 'component' : created.nodetype;
      if (lv) G().showType(lv);
      toast(`Created ${created.name || 'the node'}${created.path ? ` — ${created.path}` : ''}`, { kind: 'ok' });
      await refresh(created.id);
    }
  }

  // ------------------------------------------------------------------ configure a node
  async function configureNode(n) {
    if (isReq(n)) return editReq(n);
    let fresh = n;
    try { const r = await api('/graph/nodes/' + enc(n.id)); const one = IterGraph.normalize({ nodes: [r.node || r], edges: [] }).nodes[0]; if (one) fresh = one; } catch (e) { /* the drawing's copy will do */ }
    const ro = !S.canEdit;
    const FS = IterGraph.FILE_STATES;
    const fmt = (t) => esc(t || '—');
    const fields = [
      { key: 'name', value: fresh.name, type: 'text', group: 'Node' },
      { key: 'desc', label: 'desc', value: fresh.desc, type: 'textarea', rows: 3, group: 'Node', help: '~100 words: enough for an agent to decide whether to read the whole file' },
      { key: 'teststate', value: fresh.teststate || 'inherit', type: 'select', options: ['inherit', 'include', 'omit', 'block'], group: 'Node', help: 'inherit = as its parent · include / omit from the test sweep · block = never tested' },
    ];
    if (fresh.nodetype === 'code') fields.push({ key: 'level', value: fresh.level, type: 'select', options: IterGraph.CODE_LEVELS, group: 'Node' });
    fields.push({ key: 'body', value: fresh.body, type: 'markdown', rows: 12, group: 'Body (markdown)' });
    const front = fresh.front || {};
    Object.keys(front).sort().forEach((k) => {
      const v = front[k];
      const ro2 = k === 'last_result';
      fields.push({ key: 'front.' + k, label: k, value: v, group: 'Frontmatter (' + typeLabel(fresh).toLowerCase() + ')', removable: !ro2, readOnly: ro2, type: ro2 ? 'readonly' : (Array.isArray(v) && v.every((x) => typeof x === 'string') ? 'list' : undefined),
        help: { connects: '{"from": [supplier paths], "to": [reached paths]}', flowmap: 'summary, sequence, process_flow, data_flow', file_naming: 'sequence | uuid12', scandirs: 'where node files are looked for' }[k] });
    });
    const ch = fresh.children || {};
    ['codedirs', 'codenodes', 'tests', 'reqs'].concat(Object.keys(ch).filter((k) => !['codedirs', 'codenodes', 'tests', 'reqs'].includes(k)))
      .forEach((k) => fields.push({ key: 'children.' + k, label: k, value: Array.isArray(ch[k]) ? ch[k] : [], type: 'list', group: 'Children (paths or globs, one per line)',
        help: { codedirs: 'code folders read, changed and locked; {thisfiledir}/** = this folder down', codenodes: 'child code nodes (cascade testing / ownership)', tests: fresh.nodetype === 'test' ? 'the test scripts' : 'test.iter.md files', reqs: 'bizreq / techreq / philosophy files or folders' }[k] }));
    const meta = [['type', esc(typeLabel(fresh))], ['file', `<span class="g-mono">${fmt(fresh.path)}</span>`], ['sync', `<span class="g-fs g-fs-${esc(fresh.file_state)}">${esc(FS[fresh.file_state].label)}</span>`],
      ['creator', fmt(fresh.creator)], ['created', fmt(fresh.timestamps.create)], ['modified', fmt(fresh.timestamps.last_modified)], ['version', fresh.node_version != null ? 'v' + esc(fresh.node_version) : '—'], ['id', `<span class="g-mono">${esc(fresh.id)}</span>`]];
    return K().configure({
      title: `Configure — ${fresh.name}`, sub: `${typeLabel(fresh)} node · saved to its file by an engine`, meta, fields, readOnly: ro, allowAdd: !ro,
      note: ro ? 'Read only.' : 'New keys you add become frontmatter keys. Children entries are paths or globs: <code>{topdir}</code>, <code>{thisfiledir}</code>, <code>{thisfilestem}</code> work; a relative path is relative to this file\'s folder.',
      onSave: async ({ values, changed, removed, added }) => {
        const patch = {};
        if (fresh.node_version != null) patch.expect_version = fresh.node_version;
        ['name', 'desc', 'teststate', 'level', 'body'].forEach((k) => { if (k in changed) patch[k] = changed[k]; });
        const frontKeys = Object.keys(values).filter((k) => k.startsWith('front.') || added.includes(k));
        const frontTouched = Object.keys(changed).some((k) => k.startsWith('front.') || added.includes(k)) || removed.some((k) => k.startsWith('front.'));
        if (frontTouched) {
          const nf = {};
          Object.keys(front).forEach((k) => { if (!removed.includes('front.' + k)) nf[k] = front[k]; });
          frontKeys.forEach((k) => { nf[k.startsWith('front.') ? k.slice(6) : k] = values[k]; });
          removed.filter((k) => k.startsWith('front.')).forEach((k) => { nf[k.slice(6)] = null; }); // the server merges front: null removes a key
          patch.front = nf;
        }
        if (Object.keys(changed).some((k) => k.startsWith('children.'))) {
          const nc = Object.assign({}, ch);
          Object.keys(values).filter((k) => k.startsWith('children.')).forEach((k) => { nc[k.slice(9)] = values[k]; });
          patch.children = nc;
        }
        const keys = Object.keys(patch).filter((k) => k !== 'expect_version');
        if (!keys.length) return true;
        if (patch.name !== undefined && !String(patch.name).trim()) throw new Error('name: a node needs a name');
        try { await api('/graph/nodes/' + enc(fresh.id), { method: 'PATCH', body: JSON.stringify(patch) }); }
        catch (e) { if (e.status === 409) throw new Error('Someone (or an engine) changed this node since you opened it. Close and open Configure again to see the new version.'); throw new Error(errMsg(e)); }
        toast(`Saved ${patch.name || fresh.name}`, { kind: 'ok' });
        refresh(fresh.id);
        return true;
      },
    });
  }

  // ------------------------------------------------------------------ edges: configure, copy/paste, move, remove, draw
  function configureEdge(r) {
    const f = node(r.from); const t = node(r.to); if (!f || !t) return;
    const kinds = KIND_ORDER.filter((k) => RULES[k](f, t));
    if (!kinds.includes(r.kind)) kinds.unshift(r.kind);
    const nodes = S.data.nodes.filter((x) => !isReq(x) || x.id === r.to).sort((a, b) => a.name.localeCompare(b.name));
    const opt = (n) => ({ value: n.id, label: `${n.name} — ${typeLabel(n)}` });
    const ownerName = r.kind === 'supplies' ? t.name : f.name;
    return K().configure({
      title: `Edge — ${r.kind}`, sub: `${f.name} ${VERB[r.kind] || r.kind} ${t.name}`, readOnly: !S.canEdit,
      meta: [['kind', esc(r.kind)], ['from', esc(f.name)], ['to', esc(t.name)], ['recorded in', esc(ownerName) + '\'s file']],
      note: 'A project-graph edge is a line in a node file\'s frontmatter, so it has no settings of its own: change its kind or either end here (or drag an end in the drawing).',
      fields: [
        { key: 'kind', value: r.kind, type: 'select', options: kinds.map((k) => ({ value: k, label: `${k} — ${VERB[k]}` })) },
        { key: 'from', value: r.from, type: 'select', options: nodes.map(opt) },
        { key: 'to', value: r.to, type: 'select', options: nodes.map(opt) },
      ],
      onSave: async ({ changed }) => {
        if (!Object.keys(changed).length) return true;
        const nk = changed.kind || r.kind; const nf = changed.from || r.from; const nt = changed.to || r.to;
        const F = node(nf); const T = node(nt);
        if (nf === nt) throw new Error('An edge cannot join a node to itself.');
        if (!RULES[nk] || !RULES[nk](F, T)) throw new Error(`A "${nk}" edge cannot join a ${typeLabel(F).toLowerCase()} to a ${typeLabel(T).toLowerCase()}.`);
        if (changed.kind) {
          await api('/graph/edges', { method: 'DELETE', body: JSON.stringify({ from: r.from, to: r.to, kind: r.kind, reason: `kind changed to ${nk} in the graph` }) });
          await api('/graph/edges', { method: 'POST', body: JSON.stringify({ from: nf, to: nt, kind: nk }) });
        } else {
          const body = { from: r.from, to: r.to, kind: r.kind }; if (nf !== r.from) body.new_from = nf; if (nt !== r.to) body.new_to = nt;
          await api('/graph/edges/move', { method: 'POST', body: JSON.stringify(body) });
        }
        toast('Edge saved', { kind: 'ok' });
        await refresh();
        G().focusEdgeId(`e:${nf}|${nk}|${nt}`);
        return true;
      },
    });
  }
  function copyEdge(r) {
    if (r.kind === 'contains') { toast('A "contains" line is a section of a file: it cannot be copied. Use Move to… or Add requirement.', { kind: 'warn', ms: 5000 }); return; }
    K().clip.project = { kind: r.kind, from: r.from, to: r.to };
    toast(`Copied the "${r.kind}" edge ${nameOf(r.from)} → ${nameOf(r.to)}. Select a node and press ${K().MOD}V to paste it there.`, { kind: 'info', ms: 4500 });
    renderBar(true);
  }
  async function pasteEdge(n) {
    const c = K().clip.project; if (!c || !S.canEdit) return;
    const f = node(c.from); const t = node(c.to);
    const opts = [];
    if (t && n.id !== c.to && RULES[c.kind](n, t)) opts.push({ value: { from: n.id, to: c.to }, label: `${n.name} ${VERB[c.kind]} ${t.name}`, desc: `replaces the start (${f ? f.name : c.from})` });
    if (f && n.id !== c.from && RULES[c.kind](f, n)) opts.push({ value: { from: c.from, to: n.id }, label: `${f.name} ${VERB[c.kind]} ${n.name}`, desc: `replaces the end (${t ? t.name : c.to})` });
    if (!opts.length) { toast(`A copied "${c.kind}" edge cannot attach to ${n.name} (${typeLabel(n).toLowerCase()}).`, { kind: 'err' }); return; }
    const pick = opts.length === 1 ? opts[0].value : await K().choose({ title: `Paste "${c.kind}" edge`, text: `Which end does <b>${esc(n.name)}</b> take?`, options: opts, okLabel: 'Paste' });
    if (!pick) return;
    await addEdge(pick.from, pick.to, c.kind);
  }
  async function addEdge(from, to, kind) {
    try { await api('/graph/edges', { method: 'POST', body: JSON.stringify({ from, to, kind }) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return false; }
    toast(`${nameOf(from)} ${VERB[kind] || kind} ${nameOf(to)}`, { kind: 'ok' });
    [from, to].forEach((x) => { const nn = node(x); if (nn) G().showType(nn.level); });
    await refresh();
    G().focusEdgeId(`e:${from}|${kind}|${to}`);
    return true;
  }
  async function moveEdge(r, end, newId) {
    if (!r) return;
    if (r.kind === 'contains') {
      // the same move as "Move to…", through graph/edges/move (spec §2.8)
      const q = node(r.to);
      try { await api('/graph/edges/move', { method: 'POST', body: JSON.stringify({ from: r.from, to: r.to, kind: 'contains', new_from: newId }) }); }
      catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
      toast(`Moved ${q ? q.key || q.title : 'the requirement'} to ${nameOf(newId)}`, { kind: 'ok' });
      await refresh();
      G().focusEdgeId(`e:${newId}|contains|${r.to}`);
      return;
    }
    const body = { from: r.from, to: r.to, kind: r.kind };
    if (end === 'source') body.new_from = newId; else body.new_to = newId;
    try { await api('/graph/edges/move', { method: 'POST', body: JSON.stringify(body) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    const nf = body.new_from || r.from; const nt = body.new_to || r.to;
    toast(`Moved: ${nameOf(nf)} ${VERB[r.kind] || r.kind} ${nameOf(nt)}`, { kind: 'ok' });
    await refresh();
    G().focusEdgeId(`e:${nf}|${r.kind}|${nt}`);
  }
  async function removeEdge(r) {
    if (!S.canEdit) return;
    const reason = await K().ask({ title: 'Remove edge', text: `<b>${esc(nameOf(r.from))}</b> ${esc(VERB[r.kind] || r.kind)} <b>${esc(nameOf(r.to))}</b> — the line comes out of ${esc(nameOf(r.kind === 'supplies' ? r.to : r.from))}'s file.`,
      label: 'Why?', help: 'Required — recorded with the change (and in the commit).', required: true, requiredMsg: 'Say why: it is recorded with the change.', multiline: true, rows: 2, okLabel: 'Remove edge', danger: true });
    if (reason == null) return;
    try { await api('/graph/edges', { method: 'DELETE', body: JSON.stringify({ from: r.from, to: r.to, kind: r.kind, reason }) }); }
    catch (e) {
      const b = e && e.body; const t = toast('Refused: ' + errMsg(e), { kind: 'err', ms: 9000 });
      if (b && b.refused === 'glob' && b.owner && node(b.owner) && t) { const a = document.createElement('a'); a.textContent = ' Configure ' + nameOf(b.owner) + '…'; a.onclick = () => configureNode(node(b.owner)); t.appendChild(a); }
      return;
    }
    G().select(null);
    toast('Edge removed', { kind: 'ok' });
    refresh();
  }
  function startDraw(n) {
    if (!S.canEdit) return;
    const cy = G().cy; cancelDraw();
    const src = cy.getElementById(n.id); if (src.empty()) return;
    const p = src.position();
    cy.add([{ group: 'nodes', data: Object.assign({}, K().HELPER_NODE, { id: '__draw_cursor' }), position: { x: p.x + 40, y: p.y + 40 }, classes: 'g-draw-cursor', selectable: false, grabbable: false },
      { group: 'edges', data: Object.assign({}, K().HELPER_EDGE, { id: '__draw_edge', source: n.id, target: '__draw_cursor' }), classes: 'g-draw-edge', selectable: false }]);
    cy.style().selector('.g-draw-cursor').style({ width: 6, height: 6, 'background-color': '#4da3ff', label: '', events: 'no' })
      .selector('.g-draw-edge').style({ 'line-color': '#4da3ff', 'line-style': 'dashed', width: 2, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#4da3ff', 'curve-style': 'straight', opacity: 1, events: 'no' }).update();
    const move = (e) => { const c = cy.getElementById('__draw_cursor'); if (c.nonempty()) c.position(e.position); };
    cy.on('mousemove', move);
    DRAW = { from: n, move };
    S.root.querySelector('#g-cywrap').classList.add('g-drawing');
    G().banner(`Connecting from <b>${esc(n.name)}</b>: click the node it should reach. <kbd>Esc</kbd> cancels.`);
  }
  function cancelDraw() {
    const g = G();
    if (DRAW && g) { g.cy.off('mousemove', DRAW.move); g.cy.remove('#__draw_cursor, #__draw_edge'); }
    if (DRAW && g) g.banner('');
    DRAW = null;
    if (S) { const w = S.root.querySelector('#g-cywrap'); if (w) w.classList.remove('g-drawing'); }
  }
  async function finishDraw(targetId) {
    const a = DRAW.from; cancelDraw();
    const b = node(targetId); if (!b || b.id === a.id) return;
    const opts = [];
    // the direction drawn first, then the other way round
    KIND_ORDER.forEach((k) => { if (RULES[k](a, b)) opts.push({ value: { from: a.id, to: b.id, kind: k }, label: `${a.name} ${VERB[k]} ${b.name}`, desc: `${k} edge` }); });
    KIND_ORDER.forEach((k) => { if (RULES[k](b, a)) opts.push({ value: { from: b.id, to: a.id, kind: k }, label: `${b.name} ${VERB[k]} ${a.name}`, desc: `${k} edge (the other way round)` }); });
    if (!opts.length) { toast(`No edge kind joins a ${typeLabel(a).toLowerCase()} and a ${typeLabel(b).toLowerCase()}.`, { kind: 'err', ms: 5000 }); return; }
    const exists = (o) => S.data.edges.some((e) => e.from === o.from && e.to === o.to && e.kind === o.kind);
    const fresh = opts.filter((o) => !exists(o.value));
    if (!fresh.length) { toast('That edge is already there.', { kind: 'warn' }); return; }
    const pick = fresh.length === 1 ? fresh[0].value : await K().choose({ title: 'Connect', text: 'What does this edge mean?', options: fresh, okLabel: 'Add edge' });
    if (pick) await addEdge(pick.from, pick.to, pick.kind);
  }

  // ------------------------------------------------------------------ node actions
  async function deleteNode(n) {
    if (isReq(n)) return deleteReq(n);
    if (!S.canEdit || n.nodetype === 'project') return;
    const kids = S.data.edges.filter((e) => e.from === n.id).length; const parents = S.data.edges.filter((e) => e.to === n.id).length;
    const reason = await K().ask({ title: `Delete ${n.name}?`, text: `The ${esc(typeLabel(n).toLowerCase())} node and its file <code>${esc(n.path || '')}</code> go; ${parents ? `${parents} node(s) that list it drop the line` : 'nothing lists it'}${kids ? `, and its ${kids} link(s) out go with it (the nodes they reach stay)` : ''}. Code folders are not touched.`,
      label: 'Why?', help: 'Required — recorded with the change (and in the commit).', required: true, requiredMsg: 'Say why: it is recorded with the change.', multiline: true, rows: 2, okLabel: 'Delete node', danger: true });
    if (reason == null) return;
    try { await api('/graph/nodes/' + enc(n.id), { method: 'DELETE', body: JSON.stringify({ reason }) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    G().select(null);
    toast(`${n.name} deleted — the engine removes the file`, { kind: 'ok' });
    refresh();
  }
  async function moveFile(n) {
    const path = await K().ask({ title: `Move ${n.name}'s file`, text: 'The node keeps its id; only its file moves (git mv on the engine).', label: 'New path', value: n.path, help: 'Starts with <code>{topdir}/</code> and ends with <code>.' + esc(n.nodetype) + '.iter.md</code>.', required: true, okLabel: 'Move' });
    if (path == null || path === n.path) return;
    try { await api('/graph/nodes/' + enc(n.id) + '/move', { method: 'POST', body: JSON.stringify({ path }) }); }
    catch (e) { toast('Refused: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    toast('Move queued for the engine', { kind: 'ok' }); refresh(n.id);
  }
  async function runTests(n) {
    if (!S.canEdit || !canTest(n)) return;
    let r;
    try { r = await api('/graph/run_tests', { method: 'POST', body: JSON.stringify({ node: n.id }) }); }
    catch (e) { toast('Could not queue the tests: ' + errMsg(e), { kind: 'err', ms: 6000 }); return; }
    const id = r && (r.id || (r.item && r.item.id) || (r.workitem && r.workitem.id));
    const t = toast(`Queued a test run for <b>${esc(n.name)}</b>${id ? ` — <a data-open="${esc(id)}">work item …${esc(String(id).slice(-12))}</a>` : ''}. The engine runs it and the result lands on the test nodes.`, { kind: 'ok', html: true, ms: 7000 });
    const a = t && t.querySelector('[data-open]'); if (a && S.ctx.openItem) a.onclick = () => S.ctx.openItem(a.dataset.open);
  }

  // ------------------------------------------------------------------ keys
  function onKey(ev) {
    if (!S || !S.root.offsetParent || !G()) return;
    if (K().dialogOpen()) return;
    if (ev.key === 'Escape' && DRAW) { ev.preventDefault(); cancelDraw(); return; }
    if (K().typing()) return;
    const sel = G().selected; const sn = sel && sel.kind === 'node' ? node(sel.id) : null;
    const se = sel && sel.kind === 'edge' ? linkOf(G().cy.getElementById(sel.id)) : null;
    const mod = ev.metaKey || ev.ctrlKey;
    if (mod && !ev.altKey && !ev.shiftKey && (ev.key === 'c' || ev.key === 'C')) { if (se) { ev.preventDefault(); copyEdge(se); } return; }
    if (mod && !ev.altKey && !ev.shiftKey && (ev.key === 'v' || ev.key === 'V')) { if (sn && K().clip.project) { ev.preventDefault(); pasteEdge(sn); } return; }
    if (mod || ev.altKey) return;
    const k = ev.key.toLowerCase();
    if (k === 'e' || ev.key === 'Enter') { if (sn) { ev.preventDefault(); configureNode(sn); } else if (se) { ev.preventDefault(); configureEdge(se); } return; }
    if (!S.canEdit) return;
    if (k === 'n') { ev.preventDefault(); if (isReq(sn)) addReq({ file: sn.file, after: sn.id }); else newNode(sn); }
    else if (k === 'c' && sn) { ev.preventDefault(); startDraw(sn); }
    else if (k === 't' && sn) { ev.preventDefault(); runTests(sn); }
    else if ((ev.key === 'Delete' || ev.key === 'Backspace') && (sn || se)) { ev.preventDefault(); if (sn) deleteNode(sn); else if (se.kind === 'contains') deleteReq(node(se.to)); else removeEdge(se); }
  }
}());
