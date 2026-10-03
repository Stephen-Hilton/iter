/*
 * iter5 Project graph — the map of one project, drawn from
 * GET /api/projects/{p}/graph ({nodes:[Node], edges:[{from, kind, to}]}).
 * Every node is one *.iter.md file (nodes = files, spec §3): project, code
 * (level context | container | component | connection), test, bizreq,
 * techreq, philosophy, usecase, actor. Every edge is derived from a node's
 * frontmatter (§2.4): codenodes, tests, reqs, supplies, connects, drives,
 * touches, uses.
 *
 * Ported from iter4's viewer (itself from pdy-dev's usecase_map): the layout
 * maths ("tiered", "rings", "sequence" are position maths; "cluster" is fcose
 * over compound nodes — a code node sits inside the code node that lists it
 * in children.codenodes; "flow" is dagre, left to right), the search, the
 * wheel/trackpad handling and the use-case step views (a use case's
 * front.flowmap). New in iter5: node-type filter chips (in the URL hash, with
 * presets such as "Network map"), connection nodes, file-sync state on every
 * node, and update() — a redraw after an edit keeps every node where it was.
 *
 * Editing lives in graphedit.js (IterGraphEdit); it decorates the detail pane
 * through the "g-detail" event and drives the drawing through IterGraph.api.
 */
(function () {
  'use strict';
  let current = null; // {cy, abort, api} of the drawing on screen
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // ------------------------------------------------------------------ the visual encoding (one table, read by the stylesheet, the chips and the legend)
  const CODE_LEVELS = ['context', 'container', 'component', 'connection'];
  const LEVELS = {
    project: { label: 'Project', long: 'Project (the whole repository)', color: '#475569', border: '#94a3b8', shape: 'octagon', w: 46, h: 46, font: 15 },
    context: { label: 'Context', long: 'Context (a major area of the system)', color: '#7c3aed', border: '#c4b5fd', shape: 'round-rectangle', w: 46, h: 30, font: 14 },
    container: { label: 'Container', long: 'Container (one deployable program)', color: '#2563eb', border: '#93c5fd', shape: 'round-rectangle', w: 32, h: 22, font: 12 },
    component: { label: 'Component', long: 'Component (a part inside a program)', color: '#0d9488', border: '#5eead4', shape: 'round-rectangle', w: 24, h: 16, font: 11 },
    connection: { label: 'Connection', long: 'Connection (a type of link: API call, event, stream…)', color: '#0891b2', border: '#67e8f9', shape: 'diamond', w: 22, h: 22, font: 11.5 },
    test: { label: 'Test', long: 'Test (metadata for test scripts)', color: '#15803d', border: '#4ade80', shape: 'round-triangle', w: 18, h: 16, font: 10.5 },
    bizreq: { label: 'Bizreq', long: 'Business requirements file (one ## section per requirement)', color: '#a16207', border: '#facc15', shape: 'cut-rectangle', w: 18, h: 13, font: 10.5 },
    techreq: { label: 'Techreq', long: 'Technical requirements file (one ## section per requirement)', color: '#c2410c', border: '#fdba74', shape: 'cut-rectangle', w: 18, h: 13, font: 10.5 },
    req: { label: 'Req', long: 'Requirement (one section of a bizreq/techreq file)', color: '#8a6a1c', border: '#e5c25a', shape: 'round-rectangle', w: 11, h: 8, font: 8.5 },
    philosophy: { label: 'Philosophy', long: 'Philosophy (the highest-level guide)', color: '#a21caf', border: '#f0abfc', shape: 'star', w: 26, h: 26, font: 12 },
    usecase: { label: 'Use case', long: 'Use case (one journey)', color: '#db2777', border: '#f9a8d4', shape: 'round-tag', w: 36, h: 28, font: 13 },
    actor: { label: 'Actor', long: 'Actor (a person or outside system)', color: '#d97706', border: '#fcd34d', shape: 'ellipse', w: 30, h: 30, font: 12.5 },
  };
  const LEVEL_ORDER = ['project', 'context', 'container', 'component', 'connection', 'test', 'bizreq', 'techreq', 'req', 'philosophy', 'usecase', 'actor'];
  /** A requirement section takes its colours from its file's type (spec §2.8). */
  const REQ_STYLE = {
    bizreq: { color: '#8a6a1c', border: '#facc15', text: '#f3dc8c' },
    techreq: { color: '#9a3c14', border: '#fdba74', text: '#fbc9a0' },
  };
  const REQ_STATUS = ['draft', 'agreed', 'done'];
  const isReqFile = (n) => !!n && (n.nodetype === 'bizreq' || n.nodetype === 'techreq');
  const EDGE_KINDS = {
    codenodes: { label: 'owns (child code node)', verb: 'owns', color: '#8b95a7', line: 'dotted', width: 1.7 },
    tests: { label: 'tested by', verb: 'is tested by', color: '#22c55e', line: 'dashed', width: 1.3 },
    reqs: { label: 'requirement', verb: 'must meet', color: '#eab308', line: 'dotted', width: 1.3 },
    supplies: { label: 'supplies a connection', verb: 'supplies', color: '#06b6d4', line: 'solid', width: 2 },
    connects: { label: 'connection reaches', verb: 'connects to', color: '#22d3ee', line: 'solid', width: 2.4 },
    drives: { label: 'actor drives use case', verb: 'drives', color: '#f59e0b', line: 'solid', width: 1.6 },
    touches: { label: 'actor touches part', verb: 'touches', color: '#fbbf24', line: 'dashed', width: 1.4 },
    uses: { label: 'use case uses part', verb: 'uses', color: '#f472b6', line: 'solid', width: 1.3 },
    contains: { label: 'file holds requirement', verb: 'contains', color: '#b49a5a', line: 'solid', width: 0.8 },
  };
  const FILE_STATES = {
    synced: { label: 'synced', cls: '', glyph: '' },
    pending_write: { label: 'pending sync', cls: 'fs-pending', glyph: '⟳ ' },
    pending_delete: { label: 'pending delete', cls: 'fs-delete', glyph: '✕ ' },
    designed: { label: 'designed', cls: 'fs-designed', glyph: '✎ ' },
  };
  const PRESETS = [
    { key: 'all', label: 'All', tip: 'Every node type (requirement sections stay inside their files: switch on the Req chip to draw them)', show: LEVEL_ORDER.filter((k) => k !== 'req'), optional: ['req'] },
    { key: 'arch', label: 'Architecture', tip: 'The project and its code: contexts, containers, components and connections', show: ['project', 'context', 'container', 'component', 'connection'], layout: 'cluster' },
    { key: 'network', label: 'Network map', tip: 'Code and connections only, left to right: what supplies each connection and what it reaches', show: ['context', 'container', 'component', 'connection'], layout: 'flow' },
    { key: 'reqs', label: 'Requirements', tip: 'The project, its code, every requirements file and the philosophy (tick "sections" to draw each requirement too)', show: ['project', 'context', 'container', 'component', 'connection', 'bizreq', 'techreq', 'philosophy'], optional: ['req'] },
    { key: 'journeys', label: 'Use cases', tip: 'Actors, the use cases they drive and the parts they use', show: ['project', 'context', 'container', 'component', 'usecase', 'actor'] },
    { key: 'tests', label: 'Tests', tip: 'Code and its test nodes', show: ['project', 'context', 'container', 'component', 'connection', 'test'] },
  ];

  /** The server's graph → the shape this page draws. Tolerates iter4's {vertices} and missing fields. */
  function normalize(raw) {
    raw = raw || {};
    const nodes = []; const seen = new Set();
    for (const n of (raw.nodes || raw.vertices || [])) {
      if (!n || !n.id || n.deleted || seen.has(n.id)) continue;
      let nt = String(n.nodetype || '').toLowerCase();
      if (nt === 'main') nt = 'project';
      if (nt === 'tests' || nt === 'testgroup') nt = 'test';
      if (!LEVELS[nt] && nt !== 'code') continue; // interfaces, agent memory, unknown
      const front = (n.front && typeof n.front === 'object') ? Object.assign({}, n.front) : {};
      if (!n.front && n.flowmap) front.flowmap = n.flowmap;
      const lv = String(n.level || front.level || '').toLowerCase();
      const level = nt === 'code' ? (CODE_LEVELS.includes(lv) ? lv : 'component') : nt;
      const path = String(n.path || '');
      seen.add(n.id);
      if (nt === 'req') { nodes.push(reqNode(n, front)); continue; }
      nodes.push({
        id: String(n.id), nodetype: nt, level, path,
        name: n.name || (path.split('/').pop() || '').replace(/\.[a-z]+\.iter\.md$/, '') || String(n.id),
        desc: n.desc != null ? n.desc : (n.description || ''), body: n.body || '',
        creator: n.creator || '', teststate: n.teststate || 'inherit',
        children: (n.children && typeof n.children === 'object') ? n.children : {}, front,
        timestamps: (n.timestamps && typeof n.timestamps === 'object') ? n.timestamps : {},
        file_state: FILE_STATES[n.file_state] ? n.file_state : 'synced',
        node_version: n.node_version, file_version: n.file_version, updated_by: n.updated_by || '',
        test: n.test || front.last_result || null, raw: n,
      });
    }
    deriveReqs(nodes, raw);
    const ids = new Set(nodes.map((n) => n.id));
    const KMAP = { bizreqs: 'reqs', techreqs: 'reqs', testgroups: 'tests' };
    const edges = []; const eseen = new Set();
    for (const e of (raw.edges || [])) {
      if (!e) continue;
      const from = String(e.from != null ? e.from : e.source || ''); const to = String(e.to != null ? e.to : e.target || '');
      let kind = String(e.kind || e.type || ''); kind = KMAP[kind] || kind;
      if (!EDGE_KINDS[kind] || !ids.has(from) || !ids.has(to) || from === to) continue;
      const id = `${from}|${kind}|${to}`;
      if (eseen.has(id)) continue;
      eseen.add(id);
      edges.push({ id, from, kind, to });
    }
    // a req node always hangs off its file, whether or not the server listed the edge
    for (const n of nodes) {
      if (n.nodetype !== 'req' || !n.file || !ids.has(n.file)) continue;
      const id = `${n.file}|contains|${n.id}`;
      if (!eseen.has(id)) { eseen.add(id); edges.push({ id, from: n.file, kind: 'contains', to: n.id }); }
    }
    return { project: raw.project || '', nodes, edges, stats: raw.stats || null, iter4: !raw.nodes && !!raw.vertices };
  }

  /** A req node as the server sends it (spec §2.8): {id, key, title, status, text, order, file, file_type, path, file_state}. */
  function reqNode(n, front) {
    const f = front || {};
    const pick = (k) => (n[k] != null ? n[k] : f[k]);
    const key = String(pick('key') || ''); const title = String(pick('title') || n.name || '');
    const ft = String(pick('file_type') || '').toLowerCase();
    const status = String(pick('status') || 'draft').toLowerCase();
    const path = String(n.path || '');
    const fileId = String(pick('file') || '');
    return {
      id: String(n.id), nodetype: 'req', level: 'req', path,
      name: n.name || (key ? `${key} — ${title}` : title) || String(n.id),
      desc: n.desc || '', body: '', creator: '', teststate: 'inherit', children: {}, front: {},
      timestamps: {}, file_state: FILE_STATES[n.file_state || n.file_state_of] ? (n.file_state || n.file_state_of) : 'synced',
      node_version: undefined, file_version: undefined, updated_by: '', test: null, raw: n,
      key, title, status: REQ_STATUS.includes(status) ? status : (status || 'draft'), text: String(pick('text') || ''),
      order: Number.isFinite(+pick('order')) ? +pick('order') : 0, file: fileId, file_type: ft === 'techreq' ? 'techreq' : ft === 'bizreq' ? 'bizreq' : '',
    };
  }

  /** The sections of a requirements file body (spec §2.8): one "## " heading each, a marker line with id and status. */
  function parseReqBody(body) {
    const lines = String(body || '').replace(/\r\n?/g, '\n').split('\n');
    const secs = []; let cur = null; let fence = false;
    for (const line of lines) {
      if (/^\s*(```|~~~)/.test(line)) fence = !fence;
      const h = !fence && line.match(/^##[ \t]+(.*?)\s*$/);
      if (h) { cur = { heading: h[1], lines: [] }; secs.push(cur); continue; }
      if (cur) cur.lines.push(line);
    }
    return secs.map((sct, i) => {
      const m = sct.heading.match(/^([A-Za-z0-9_.\-]+)\s+[—–-]\s+(.*)$/);
      let id = ''; let status = 'draft';
      const keep = [];
      for (const l of sct.lines) {
        const mk = l.match(/^\s*<!--\s*req:(.*?)-->\s*$/);
        if (mk && !id) {
          const mi = mk[1].match(/\bid=([^\s]+)/); const ms = mk[1].match(/\bstatus=([^\s]+)/);
          if (mi) id = mi[1]; if (ms) status = ms[1].toLowerCase();
          continue;
        }
        keep.push(l);
      }
      return { id, key: m ? m[1] : '', title: m ? m[2] : sct.heading, status, text: keep.join('\n').trim(), order: i };
    });
  }

  /**
   * Every requirements file gets .reqs (its sections, in order). The server derives req nodes (§2.8); when it
   * has not (an older server, or a graph/view-shaped file node carrying reqs[]), they are derived here from
   * the file's body or its reqs[] summary, so the table and the drawing work either way.
   */
  function deriveReqs(nodes, raw) {
    const serverReqs = nodes.some((n) => n.nodetype === 'req');
    const files = nodes.filter(isReqFile);
    const have = new Set(nodes.map((n) => n.id));
    if (!serverReqs) {
      for (const f of files) {
        const rawF = f.raw || {};
        let secs = [];
        if (f.body && /^##[ \t]/m.test(f.body)) secs = parseReqBody(f.body);
        else if (Array.isArray(rawF.reqs)) secs = rawF.reqs.map((r, i) => Object.assign({ order: i, text: '' }, r));
        secs.forEach((r, i) => {
          const id = r.id && !have.has(r.id) ? String(r.id) : `${f.id}#${i}`;
          have.add(id);
          const n = reqNode({ id, key: r.key, title: r.title, status: r.status, text: r.text, order: r.order != null ? r.order : i, file: f.id, file_type: f.nodetype, path: `${f.path}#${id}`, file_state: f.file_state }, {});
          n.derived = !r.id; // no id yet: the server gives it one on the next write
          nodes.push(n);
        });
      }
    }
    const byFile = new Map();
    for (const n of nodes) {
      if (n.nodetype !== 'req') continue;
      if (!n.file_type) { const f = nodes.find((x) => x.id === n.file); if (f && isReqFile(f)) n.file_type = f.nodetype; }
      if (!byFile.has(n.file)) byFile.set(n.file, []);
      byFile.get(n.file).push(n);
    }
    for (const f of files) {
      f.reqs = (byFile.get(f.id) || []).sort((a, b) => a.order - b.order || a.title.localeCompare(b.title));
      const rc = f.raw && f.raw.req_count;
      f.req_count = f.reqs.length || (Number.isFinite(+rc) ? +rc : 0);
    }
  }

  const TEMPLATE = `
<div class="g-bar">
  <div class="g-row g-toprow">
    <div class="g-group">
      <select id="g-ucpick" aria-label="View" title="Whole project, one use case's journey, or the Settings graph"></select>
      <label class="g-chk ucopt" title="Keep the rest of the project on screen, faded, instead of hiding it"><input type="checkbox" id="g-dim"> dim the rest</label>
    </div>
    <div class="g-group">
      <div class="g-seg" id="g-layouts" role="group" aria-label="Layout">
        <button data-layout="cluster" title="Boxes inside boxes: each code node holds the code nodes it owns (force-directed)">Cluster</button>
        <button data-layout="tiered" title="Rows from top to bottom by type (or, in a use case, by the order the journey reaches each part)">Tiered</button>
        <button data-layout="rings" title="Rings from the centre outward">Rings</button>
        <button data-layout="flow" title="Left to right, following the arrows (dagre) — the Network map uses it">Flow</button>
        <button data-layout="sequence" title="One use case only: one row per step, top to bottom">Sequence</button>
      </div>
    </div>
    <div class="g-group ucopt">
      <div class="g-seg" id="g-flowkind" role="group" aria-label="Flow">
        <button data-flow="process" title="Who calls or triggers whom, in order. Steps 1, 2, 3 …">Process</button>
        <button data-flow="data" title="Where the information travels. Steps D1, D2, D3 …">Data</button>
        <button data-flow="both" title="Both flows at once">Both</button>
      </div>
      <select id="g-branch" class="hidden" aria-label="Branch"></select>
    </div>
    <div class="g-group g-searchgrp">
      <div class="g-searchbox">
        <input type="search" id="g-search" placeholder="Search nodes and steps" aria-label="Search nodes and steps" autocomplete="off" spellcheck="false">
        <kbd class="g-kbdhint">/</kbd>
        <div id="g-searchlist" class="hidden"></div>
      </div>
    </div>
    <div class="g-group g-tools">
      <button class="g-btn" id="g-fit" title="Fit the whole drawing on screen (F)">Fit</button>
      <button class="g-btn" id="g-relayout" title="Run the current layout again (R)">Re-layout</button>
      <button class="g-btn hidden" id="g-showhidden" title="Put back every node you hid with Hide this node">Show hidden</button>
      <button class="g-btn g-icon" id="g-help" title="Keyboard shortcuts and how to edit (?)" aria-label="Help">?</button>
    </div>
  </div>
  <div class="g-row g-chiprow">
    <span class="g-lbl">Show</span>
    <div class="g-chips" id="g-chips"></div>
    <span class="g-sep"></span>
    <div class="g-presets" id="g-presets"></div>
    <label class="g-chk reqsecopt" title="Draw every requirement (each ## section of a bizreq/techreq file) as a small node beside its file — the Req chip"><input type="checkbox" id="g-reqsec"> requirement sections</label>
    <label class="g-chk wholeopt" title="Draw the dotted 'owns' lines even where Cluster already shows ownership as boxes"><input type="checkbox" id="g-own"> ownership lines</label>
  </div>
  <div class="g-row g-editrow"><span id="g-summary" class="g-summary"></span><span class="g-edit" id="g-editbar"></span></div>
</div>
<div id="g-designbar" class="g-designbar hidden"></div>
<div class="g-main">
  <div id="g-cywrap">
    <div id="g-cy"></div>
    <div id="g-empty" class="hidden"></div>
    <div id="g-ucsummary" class="hidden"></div>
    <div id="g-banner" class="hidden"></div>
    <div id="g-tip" class="hidden"></div>
  </div>
  <aside id="g-detail" class="g-detail hidden" aria-label="Details"></aside>
</div>
<div id="g-legend" class="g-legend"></div>
`;

  let registered = false;
  /** fcose + dagre, once per page (the Settings graph shares them). */
  function registerLayouts() {
    if (window.__iterLayouts) return;
    window.__iterLayouts = true;
    if (typeof cytoscapeFcose !== 'undefined') cytoscape.use(cytoscapeFcose);
    if (typeof cytoscapeDagre !== 'undefined') cytoscape.use(cytoscapeDagre);
  }
  window.IterGraph = {
    /** Draw graph data (GET …/graph JSON, raw) into root; any earlier drawing is torn down. */
    draw(root, raw, ctx) {
      if (current) {
        try { current.abort.abort(); } catch (e) { /* old browser */ }
        try { current.cy && current.cy.destroy(); } catch (e) { /* already gone */ }
        current = null;
      }
      root.innerHTML = TEMPLATE;
      if (!registered) { registerLayouts(); registered = true; }
      const abort = new AbortController();
      current = { abort, cy: null, api: null };
      const G = normalize(raw);
      current.api = start(root, G, ctx || {}, abort.signal);
      current.cy = current.api.cy;
      return current.cy;
    },
    /** Redraw with fresh data, keeping every node that is still there where it was. */
    update(raw) { if (current && current.api) current.api.update(normalize(raw)); },
    get current() { return current; },
    get api() { return current && current.api; },
    registerLayouts, normalize, LEVELS, LEVEL_ORDER, EDGE_KINDS, FILE_STATES, CODE_LEVELS,
  };

  function start(ROOT, G0, CTX, SIGNAL) {
    const $ = (id) => document.getElementById('g-' + id);
    const onDoc = (type, fn) => document.addEventListener(type, fn, { signal: SIGNAL });
    const BAD = '#dc2626';
    const GOOD_STATUS = new Set(['live', 'complete']);
    const isBad = (status) => !!status && !GOOD_STATUS.has(String(status).toLowerCase());
    const stepName = (e) => (e.kind === 'data' ? 'D' : '') + (e.step == null ? '?' : String(e.step));

    // ------------------------------------------------------------------ the model (rebuilt by update())
    let G = null; let nodesById = new Map(); let outE = new Map(); let inE = new Map();
    let ucById = new Map(); let USECASES = []; let flowsByUc = new Map();
    const slugify = (s) => String(s || '').toLowerCase().replace(/[^a-z0-9_-]+/g, '_').replace(/^_+|_+$/g, '');
    const normPath = (p) => String(p || '').trim().replace(/^\{topdir\}\/?/, '').replace(/^\.\//, '').replace(/\/+$/, '');
    function index(g) {
      G = g;
      nodesById = new Map(G.nodes.map((n) => [n.id, n]));
      outE = new Map(); inE = new Map();
      for (const e of G.edges) {
        if (!outE.has(e.from)) outE.set(e.from, []); outE.get(e.from).push(e);
        if (!inE.has(e.to)) inE.set(e.to, []); inE.get(e.to).push(e);
      }
      // ownership: a code node sits inside the code node that lists it in children.codenodes
      const isOwner = (n) => n && n.nodetype === 'code' && n.level !== 'connection';
      for (const n of G.nodes) {
        n.parent = null;
        if (n.nodetype !== 'code' || n.level === 'connection') continue;
        const own = (inE.get(n.id) || []).find((e) => e.kind === 'codenodes' && isOwner(nodesById.get(e.from)));
        if (own) n.parent = own.from;
      }
      for (const n of G.nodes) { // cut ownership loops
        const seen = new Set([n.id]); let p = n.parent;
        while (p) { if (seen.has(p)) { n.parent = null; break; } seen.add(p); p = (nodesById.get(p) || {}).parent; }
      }
      // use cases and their flowmaps
      const byPath = new Map(); const byDir = new Map(); const byName = new Map(); const bySlug = new Map();
      for (const n of G.nodes) {
        const p = normPath(n.path);
        if (p) byPath.set(p, n.id);
        if (n.nodetype === 'code' && p.includes('/')) { const d = p.slice(0, p.lastIndexOf('/')); if (!byDir.has(d)) byDir.set(d, n.id); }
        const nm = String(n.name || '').toLowerCase(); if (nm && !byName.has(nm)) byName.set(nm, n.id);
        const sl = slugify(n.name); if (sl && !bySlug.has(n.level + ':' + sl)) bySlug.set(n.level + ':' + sl, n.id);
      }
      const resolve = (ref) => {
        const r = String(ref == null ? '' : ref).trim();
        if (!r) return null;
        if (nodesById.has(r)) return r;
        const p = normPath(r);
        if (byPath.has(p)) return byPath.get(p);
        if (byDir.has(p)) return byDir.get(p);
        const m = r.match(/^(actor|usecase):(.+)$/);
        if (m) return bySlug.get(m[1] + ':' + slugify(m[2])) || byName.get(m[2].toLowerCase()) || null;
        if (byName.has(r.toLowerCase())) return byName.get(r.toLowerCase());
        const tail = p.split('/').pop();
        if (tail && byDir.has(p.replace(/\/[^/]*\.iter\.md$/, ''))) return byDir.get(p.replace(/\/[^/]*\.iter\.md$/, ''));
        return null;
      };
      USECASES = []; flowsByUc = new Map();
      for (const u of G.nodes.filter((n) => n.nodetype === 'usecase')) {
        const fm = u.front.flowmap && typeof u.front.flowmap === 'object' ? u.front.flowmap : null;
        const uses = (outE.get(u.id) || []).filter((e) => e.kind === 'uses').map((e) => e.to);
        const actors = (inE.get(u.id) || []).filter((e) => e.kind === 'drives').map((e) => e.from);
        const has = !!fm && ['process_flow', 'data_flow'].some((k) => Array.isArray(fm[k]) && fm[k].length);
        const rec = { id: u.id, name: u.name, desc: u.desc, path: u.path, has_flowmap: has, summary: (fm && fm.summary) || '',
          uses, actors, sequence: [], process_steps: [], data_steps: [], branches: [], unresolved: [] };
        const flows = [];
        const seq = []; const addSeq = (id) => { if (id && !seq.includes(id)) seq.push(id); };
        if (fm) {
          (Array.isArray(fm.sequence) ? fm.sequence : []).forEach((r) => { const id = resolve(r); if (id) addSeq(id); else rec.unresolved.push(String(r)); });
          for (const [kind, key, textkey] of [['process', 'process_flow', 'what'], ['data', 'data_flow', 'data']]) {
            (Array.isArray(fm[key]) ? fm[key] : []).forEach((st, i) => {
              if (!st || typeof st !== 'object') return;
              const a = resolve(st.from); const b = resolve(st.to);
              [[st.from, a], [st.to, b]].forEach(([raw, id]) => { if (!id && raw != null && !rec.unresolved.includes(String(raw))) rec.unresolved.push(String(raw)); });
              const step = st.step == null ? null : (Number.isFinite(+st.step) ? +st.step : st.step);
              const branch = st.branch ? String(st.branch) : null;
              if (branch && !rec.branches.includes(branch)) rec.branches.push(branch);
              const r = { step, branch, status: st.status || null, via: st.via || null, plain: st.plain || null, evidence: st.evidence || null, stored: !!st.stored, [textkey]: st[textkey] || '' };
              rec[kind + '_steps'].push(r);
              if (a && b) {
                flows.push(Object.assign({ id: `flow|${u.id}|${kind}|${i}`, type: 'flow', kind, usecase: u.id, source: a, target: b }, r));
                addSeq(a); addSeq(b);
              }
            });
          }
        }
        actors.forEach(addSeq); uses.forEach(addSeq);
        rec.sequence = seq;
        USECASES.push(rec);
        flowsByUc.set(u.id, flows);
      }
      ucById = new Map(USECASES.map((u) => [u.id, u]));
      buildSearchIndex();
    }

    // ------------------------------------------------------------------ state (mirrored in the URL hash)
    const DEFAULTS = { uc: '', layout: 'cluster', flow: 'process', branch: '', dim: false, own: false, hide: null };
    const state = Object.assign({}, DEFAULTS);
    const BOOLS = ['dim', 'own'];
    const HIDE_KEY = () => 'iter5.graph.hide2.' + (CTX.project || ''); // hide2: since req nodes (2026-10-02), so an older saved choice cannot unhide them
    /** The default node types to hide: requirement sections always; tests and requirement files too on a big project. */
    function defaultHide() { return G.nodes.filter((n) => n.nodetype !== 'req').length > 160 ? ['test', 'bizreq', 'techreq', 'req'] : ['req']; }
    let HIDE_TYPES = new Set();
    function readHash() {
      const p = new URLSearchParams(location.hash.slice(1));
      Object.assign(state, DEFAULTS);
      for (const k of Object.keys(DEFAULTS)) {
        if (!p.has(k)) continue;
        state[k] = BOOLS.includes(k) ? p.get(k) === '1' : p.get(k);
      }
      if (state.uc && !ucById.has(state.uc)) state.uc = '';
      if (!['cluster', 'tiered', 'rings', 'sequence', 'flow'].includes(state.layout)) state.layout = 'cluster';
      if (!['process', 'data', 'both'].includes(state.flow)) state.flow = DEFAULTS.flow;
      let hide = state.hide;
      if (hide == null) { try { hide = localStorage.getItem(HIDE_KEY()); } catch (e) { hide = null; } }
      HIDE_TYPES = new Set(hide == null ? defaultHide() : String(hide).split(',').filter((x) => LEVELS[x]));
    }
    function writeHash() {
      const keep = new URLSearchParams(location.hash.slice(1));
      const p = new URLSearchParams();
      for (const k of ['tab', 'p']) if (keep.has(k)) p.set(k, keep.get(k));
      for (const k of Object.keys(DEFAULTS)) {
        if (k === 'hide') continue;
        if (state[k] === DEFAULTS[k]) continue;
        p.set(k, BOOLS.includes(k) ? (state[k] ? '1' : '0') : state[k]);
      }
      const hide = [...HIDE_TYPES].sort().join(',');
      if (hide !== [...defaultHide()].sort().join(',')) p.set('hide', hide || 'none');
      try { localStorage.setItem(HIDE_KEY(), hide); } catch (e) { /* storage blocked */ }
      if (p.toString() !== new URLSearchParams(location.hash.slice(1)).toString()) history.replaceState(null, '', '#' + p.toString());
    }

    // ------------------------------------------------------------------ what is on screen
    const HIDDEN = new Set();
    try { JSON.parse(sessionStorage.getItem('iter5.graph.hidden') || '[]').forEach((id) => HIDDEN.add(id)); } catch (e) { /* storage blocked */ }
    function saveHidden() { try { sessionStorage.setItem('iter5.graph.hidden', JSON.stringify([...HIDDEN])); } catch (e) { /* storage blocked */ } }
    function hideNode(id) { HIDDEN.add(id); saveHidden(); select(null); render({ keep: snapshot() }); }
    function showAllHidden() { HIDDEN.clear(); saveHidden(); render({ keep: snapshot() }); }
    // a requirement section is drawn beside its file: hidden with the file's type too
    const typeShown = (n) => n && !HIDE_TYPES.has(n.level) && !(n.nodetype === 'req' && n.file_type && HIDE_TYPES.has(n.file_type));

    function currentView() {
      const uc = state.uc ? ucById.get(state.uc) : null;
      const touched = new Set();
      let flowEdges = [];
      if (uc) {
        touched.add(uc.id);
        const onAnyStep = new Set();
        (flowsByUc.get(uc.id) || []).forEach((e) => { onAnyStep.add(e.source); onAnyStep.add(e.target); });
        uc.sequence.forEach((id) => { if (!onAnyStep.has(id)) touched.add(id); });
        flowEdges = (flowsByUc.get(uc.id) || []).filter((e) =>
          (state.flow === 'both' || e.kind === state.flow)
          && (!state.branch || !e.branch || e.branch === state.branch)
          && !HIDDEN.has(e.source) && !HIDDEN.has(e.target));
        flowEdges.forEach((e) => { touched.add(e.source); touched.add(e.target); });
      }
      const whole = !uc || state.dim;
      let ids = whole
        ? G.nodes.filter((n) => typeShown(n) || (uc && touched.has(n.id))).map((n) => n.id)
        : [...touched].filter((id) => nodesById.has(id) && (typeShown(nodesById.get(id)) || id === uc.id || uc.actors.includes(id)));
      ids = ids.filter((id) => !HIDDEN.has(id));
      const idset = new Set(ids);
      const edges = [];
      const both = (e) => idset.has(e.from) && idset.has(e.to);
      if (!uc || state.dim) {
        for (const e of G.edges) if (both(e)) edges.push({ id: 'L|' + e.id, type: 'link', kind: e.kind, source: e.from, target: e.to, link: e });
      }
      if (uc) {
        flowEdges.forEach((e) => edges.push(e));
        if (uc.has_flowmap) {
          const first = [...flowEdges].sort((a, b) => (a.step ?? 1e9) - (b.step ?? 1e9) || (a.kind === 'process' ? -1 : 1))[0];
          const startId = first ? first.source : uc.sequence[0];
          if (startId && idset.has(startId) && startId !== uc.id) edges.push({ id: 'start|' + uc.id, type: 'start', source: uc.id, target: startId });
        }
        if (!state.dim) {
          // the use case's own links: actors that drive it, parts it uses
          for (const e of (outE.get(uc.id) || []).concat(inE.get(uc.id) || [])) {
            if ((e.kind === 'uses' || e.kind === 'drives') && both(e)) edges.push({ id: 'L|' + e.id, type: 'link', kind: e.kind, source: e.from, target: e.to, link: e });
          }
        }
      }
      return { uc, ids, idset, edges, touched, flowEdges, whole };
    }

    const LABEL_CHAR_PX = 10; const LABEL_PAD_PX = 14; const MIN_GAP = 40;
    /** Use-case steps between the same two parts each get their own curve (see iter4's notes). */
    function spreadParallel(list, bow) {
      const out = list.map((m, i) => (bow[i] ? bow[i] : null));
      const groups = new Map();
      list.forEach((m, i) => {
        if (m.source === m.target) return;
        const key = m.source < m.target ? m.source + '\u0000' + m.target : m.target + '\u0000' + m.source;
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(i);
      });
      const rank = (m) => { const k = m.type === 'flow' ? (m.kind === 'process' ? 0 : 1) : m.type === 'start' ? -1 : 2; return [k, m.step ?? 1e9, m.branch || '']; };
      groups.forEach((idx) => {
        if (idx.length < 2) return;
        const dir = (i) => (list[i].source < list[i].target ? 1 : -1);
        const base = (i) => (bow[i] || 0) * dir(i);
        const widest = Math.max(...idx.map((i) => edgeVisual(list[i]).label.length));
        const gap = Math.max(MIN_GAP, 2 * (widest * LABEL_CHAR_PX + LABEL_PAD_PX));
        const fans = new Map();
        idx.forEach((i) => { const k = Math.round(base(i)); if (!fans.has(k)) fans.set(k, []); fans.get(k).push(i); });
        fans.forEach((fan, centre) => {
          const order = [...fan].sort((x, y) => { const rx = rank(list[x]); const ry = rank(list[y]); return rx[0] - ry[0] || rx[1] - ry[1] || String(rx[2]).localeCompare(String(ry[2])); });
          const outward = Math.sign(centre);
          order.forEach((i, j) => { const c = outward ? centre + outward * j * gap : centre + (j - (order.length - 1) / 2) * gap; out[i] = c * dir(i); });
        });
      });
      return out;
    }

    function edgeVisual(m) {
      let color = '#94a3b8'; let line = 'solid'; let width = 1.3; let arrow = 'triangle'; let label = ''; const cls = [m.type];
      if (m.type === 'link') {
        const k = EDGE_KINDS[m.kind] || {}; color = k.color || color; line = k.line || line; width = k.width || width; cls.push('k-' + m.kind);
      } else if (m.type === 'flow') {
        color = m.kind === 'process' ? '#3b82f6' : '#f97316'; width = 3; label = stepName(m);
        if (m.branch) { label += '*'; cls.push('branch'); }
        cls.push(m.kind);
        if (isBad(m.status)) { color = BAD; line = 'dashed'; cls.push('bad'); }
      } else if (m.type === 'start') { color = '#f472b6'; width = 2.5; label = 'starts'; }
      return { color, line, width, arrow, label, cls: cls.join(' ') };
    }

    // ------------------------------------------------------------------ layout maths (iter4)
    function groupBands(ids, bandOf, orderKey) {
      const by = new Map(); const key = orderKey || (() => 0);
      for (const id of [...ids].sort((a, b) => key(a) - key(b) || (a < b ? -1 : a > b ? 1 : 0))) { const b = bandOf(id); if (!by.has(b)) by.set(b, []); by.get(b).push(id); }
      return [...by.keys()].sort((a, b) => a - b).map((k) => by.get(k));
    }
    function adjacency(ids, edges) {
      const adj = new Map(ids.map((id) => [id, new Set()]));
      for (const e of edges) { if (e.source === e.target) continue; const a = adj.get(e.source); const b = adj.get(e.target); if (!a || !b) continue; a.add(e.target); b.add(e.source); }
      return adj;
    }
    const subRows = (n, perRow) => Math.max(1, Math.ceil(n / perRow));
    function tieredPositions(ids, edges, bandOf, opts) {
      const o = Object.assign({ colGap: 150, subRowGap: 66, tierGap: 170, aspect: 1.7, minPerRow: 6, sweeps: 8 }, opts || {});
      const pos = new Map(); if (!ids.length) return pos;
      const bands = groupBands(ids, bandOf, o.orderKey); const adj = adjacency(ids, edges);
      const bandOfId = new Map(); bands.forEach((b, i) => b.forEach((id) => bandOfId.set(id, i)));
      const sizes = bands.map((b) => b.length); const maxBand = Math.max(...sizes);
      let perRow = Math.max(1, maxBand);
      if (maxBand > o.minPerRow) {
        let best = Infinity;
        for (let m = o.minPerRow; m <= maxBand; m++) {
          let width = 1; let height = (sizes.length - 1) * o.tierGap;
          sizes.forEach((n) => { const k = subRows(n, m); width = Math.max(width, (Math.ceil(n / k) - 1) * o.colGap); height += (k - 1) * o.subRowGap; });
          const score = Math.abs(Math.log(width / Math.max(1, height) / o.aspect));
          if (score < best - 1e-9) { best = score; perRow = m; }
        }
      }
      const tops = []; let y = 0;
      bands.forEach((b) => { tops.push(y); y += (subRows(b.length, perRow) - 1) * o.subRowGap + o.tierGap; });
      const placeBand = (order, top) => {
        const k = subRows(order.length, perRow); const cols = Math.ceil(order.length / k);
        order.forEach((id, i) => { const col = Math.floor(i / k); const row = i % k; const stagger = k > 1 ? (row / (k - 1) - 0.5) * 0.5 * o.colGap : 0;
          pos.set(id, { x: (col - (cols - 1) / 2) * o.colGap + stagger, y: top + row * o.subRowGap }); });
      };
      const orders = bands.map((b) => [...b]);
      orders.forEach((ord, i) => placeBand(ord, tops[i]));
      const reorder = (i, use) => {
        const bary = new Map();
        for (const id of orders[i]) { const xs = [...(adj.get(id) || [])].filter((nb) => use(bandOfId.get(nb))).map((nb) => pos.get(nb).x); bary.set(id, xs.length ? xs.reduce((a, v) => a + v, 0) / xs.length : pos.get(id).x); }
        orders[i].sort((a, b) => bary.get(a) - bary.get(b) || (o.orderKey ? o.orderKey(a) - o.orderKey(b) : 0) || a.localeCompare(b));
        placeBand(orders[i], tops[i]);
      };
      for (let i = 1; i < bands.length; i++) reorder(i, (j) => j < i);
      for (let s = 0; s < o.sweeps; s++) { for (let i = bands.length - 2; i >= 0; i--) reorder(i, (j) => j !== i); for (let i = 1; i < bands.length; i++) reorder(i, (j) => j !== i); }
      return pos;
    }
    const TAU = Math.PI * 2;
    const norm = (a) => ((a % TAU) + TAU) % TAU;
    function circularMean(angles) {
      if (!angles.length) return null; let s = 0; let c = 0;
      angles.forEach((a) => { s += Math.sin(a); c += Math.cos(a); });
      if (Math.abs(s) < 1e-9 && Math.abs(c) < 1e-9) return null;
      return Math.atan2(s, c);
    }
    function ringPositions(ids, edges, bandOf, opts) {
      const o = Object.assign({ minArc: 150, ringGap: 170, subRingGap: 80, maxSubRings: 3, passes: 8 }, opts || {});
      const pos = new Map(); if (!ids.length) return pos;
      const bands = groupBands(ids, bandOf); const adj = adjacency(ids, edges); const angle = new Map(); const rings = []; let prevOuter = -Infinity;
      bands.forEach((rids, bi) => {
        const n = rids.length;
        if (bi === 0 && n === 1) { rings.push({ ids: rids, r: 0, k: 1, center: true }); prevOuter = 0; return; }
        const minR = prevOuter === -Infinity ? 0 : prevOuter + o.ringGap; const need = (n * o.minArc) / TAU;
        let k = 1; if (bi > 0) while (k < o.maxSubRings && need / k > minR + o.subRingGap * k) k++;
        const slots = Math.ceil(n / k) * k;
        const chordR = n > 1 ? o.minArc / (2 * Math.sin(Math.min(Math.PI / 2, (k * Math.PI) / slots))) : 0;
        const r = Math.max(minR, (slots * o.minArc) / TAU / k, chordR);
        rings.push({ ids: rids, r, k, center: false }); prevOuter = r + (k - 1) * o.subRingGap;
      });
      const setPos = (id, p) => { pos.set(id, p); if (Math.hypot(p.x, p.y) < 1e-9) angle.delete(id); else angle.set(id, Math.atan2(p.y, p.x)); };
      const place = (ring, useAll) => {
        const { r, k } = ring; const rids = ring.ids;
        if (ring.center) { setPos(rids[0], { x: 0, y: 0 }); return; }
        const n = rids.length; const target = new Map();
        rids.forEach((id, i) => { const nbs = [...(adj.get(id) || [])].filter((nb) => angle.has(nb) && (useAll || !rids.includes(nb))); const m = circularMean(nbs.map((nb) => angle.get(nb))); target.set(id, norm(m ?? angle.get(id) ?? (i / n) * TAU)); });
        const order = [...rids].sort((a, b) => target.get(a) - target.get(b) || a.localeCompare(b));
        const step = TAU / (Math.ceil(n / k) * k);
        const offset = circularMean(order.map((id, j) => target.get(id) - j * step)) ?? 0;
        order.forEach((id, j) => { const a = norm(offset + j * step); const radius = r + (j % k) * o.subRingGap; setPos(id, { x: radius * Math.cos(a), y: radius * Math.sin(a) }); });
      };
      for (const ring of rings) place(ring, false);
      for (let p = 1; p < o.passes; p++) for (const ring of rings) place(ring, true);
      return pos;
    }
    const WHOLE_TIER = { project: 0, philosophy: 1, bizreq: 1, techreq: 1, actor: 2, usecase: 2, context: 3, connection: 4, container: 5, component: 6, test: 7 };
    const WHOLE_RING = { project: 0, philosophy: 1, context: 1, container: 2, connection: 3, component: 3, bizreq: 4, techreq: 4, test: 4, usecase: 5, actor: 6 };
    const levelOf = (id) => (nodesById.get(id) || {}).level;
    function ucBands(v) {
      const b = new Map([[v.uc.id, 0]]);
      if (!v.uc.has_flowmap) { for (const id of v.ids) if (!b.has(id)) b.set(id, 1 + (WHOLE_TIER[levelOf(id)] ?? 8)); return { bands: b }; }
      const primary = state.flow === 'data' ? 'data' : 'process';
      const byStep = (a, c) => (a.step ?? 1e9) - (c.step ?? 1e9);
      let prim = v.flowEdges.filter((e) => e.kind === primary).sort(byStep); let other = v.flowEdges.filter((e) => e.kind !== primary).sort(byStep);
      if (!prim.length) { prim = other; other = []; }
      const seen = new Set(); const starts = []; const out = new Map();
      for (const e of prim) { if (!seen.has(e.source)) starts.push(e.source); seen.add(e.source); seen.add(e.target); if (!out.has(e.source)) out.set(e.source, []); out.get(e.source).push(e.target); }
      const queue = [];
      starts.forEach((id) => { if (!b.has(id)) { b.set(id, 1); queue.push(id); } });
      while (queue.length) { const id = queue.shift(); for (const nb of out.get(id) || []) if (!b.has(nb)) { b.set(nb, b.get(id) + 1); queue.push(nb); } }
      for (let pass = 0; pass < 5; pass++) for (const e of other) if (b.has(e.source) && !b.has(e.target)) b.set(e.target, b.get(e.source) + 1);
      let max = 0; b.forEach((x) => { max = Math.max(max, x); });
      for (const e of other) { if (!b.has(e.source)) b.set(e.source, 1); if (!b.has(e.target)) b.set(e.target, max + 1); }
      v.uc.sequence.forEach((id) => { if (!b.has(id)) b.set(id, ++max); });
      for (const id of v.ids) if (!b.has(id)) b.set(id, max + 1);
      return { bands: b };
    }
    const SEQ = { col: 185, row: 40, top: 70 };
    const BLANK_NODE = { label: '', level: '', color: '#ffffff', border: '#ffffff', shape: 'rectangle', w: 1, h: 1, font: 12 };
    const BLANK_EDGE = { color: '#cbd5e1', line: 'dashed', width: 1.2, arrow: 'none', label: '', cpd: 0 };
    const reqLabel = (n) => n.key || trunc(n.title || n.name, 30);
    function nodeData(n, extra) {
      const st = LEVELS[n.level] || LEVELS.component; const fs = FILE_STATES[n.file_state] || FILE_STATES.synced;
      if (n.nodetype === 'req') {
        const rs = REQ_STYLE[n.file_type] || REQ_STYLE.bizreq;
        return Object.assign({ id: n.id, label: reqLabel(n), level: 'req', color: rs.color, border: rs.border, tcolor: rs.text, shape: st.shape, w: st.w, h: st.h, font: st.font }, extra || {});
      }
      return Object.assign({ id: n.id, label: fs.glyph + n.name, level: n.level, color: st.color, border: st.border, shape: st.shape, w: st.w, h: st.h, font: st.font }, extra || {});
    }
    function nodeClasses(n) {
      if (n.nodetype === 'req') return ['req', 'ft-' + (n.file_type || 'bizreq'), 'rs-' + (REQ_STATUS.includes(n.status) ? n.status : 'draft')];
      const c = [n.level, n.nodetype === 'code' ? 'code' : ''];
      const fs = FILE_STATES[n.file_state]; if (fs && fs.cls) c.push(fs.cls);
      if (n.nodetype === 'test' && n.test && typeof n.test === 'object') {
        const ok = n.test.overall_success != null ? n.test.overall_success : (n.test.result ? n.test.result === 'green' : null);
        if (ok === true) c.push('test-pass'); else if (ok === false) c.push('test-fail');
      }
      return c.filter(Boolean);
    }
    function sequenceElements(v) {
      const els = []; const pos = new Map();
      const byStep = (a, c) => (a.step ?? 1e9) - (c.step ?? 1e9);
      const kinds = state.flow === 'both' ? ['process', 'data'] : [state.flow];
      const rows = [];
      kinds.forEach((k) => { const list = v.flowEdges.filter((e) => e.kind === k).sort(byStep); if (list.length && kinds.length > 1) rows.push({ section: k }); list.forEach((e) => rows.push({ e })); });
      const parts = []; const seen = new Set();
      rows.forEach((r) => { if (r.e) for (const id of [r.e.source, r.e.target]) if (!seen.has(id)) { seen.add(id); parts.push(id); } });
      const xOf = new Map(parts.map((id, i) => [id, i * SEQ.col]));
      const bottom = SEQ.top + rows.length * SEQ.row;
      for (const id of parts) {
        const n = nodesById.get(id);
        els.push({ group: 'nodes', classes: nodeClasses(n).concat(['focus', 'seqhead']).join(' '), data: nodeData(n) });
        pos.set(id, { x: xOf.get(id), y: 0 });
        els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: 'll|' + id }, selectable: false, grabbable: false });
        pos.set('ll|' + id, { x: xOf.get(id), y: bottom });
        els.push({ group: 'edges', classes: 'lifeline', data: { ...BLANK_EDGE, id: 'lle|' + id, source: id, target: 'll|' + id } });
      }
      rows.forEach((r, i) => {
        const y = SEQ.top + i * SEQ.row;
        if (r.section) { const sid = 'sec|' + r.section; els.push({ group: 'nodes', classes: 'section', data: { ...BLANK_NODE, id: sid, label: r.section === 'process' ? 'Process flow' : 'Data flow' }, selectable: false, grabbable: false }); pos.set(sid, { x: -SEQ.col * 0.75, y }); return; }
        const e = r.e; const a = `a|${i}|s`; const b = `a|${i}|t`; const self = e.source === e.target;
        els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: a }, selectable: false, grabbable: false });
        els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: b }, selectable: false, grabbable: false });
        pos.set(a, { x: xOf.get(e.source), y }); pos.set(b, { x: xOf.get(e.target), y: self ? y + SEQ.row * 0.55 : y });
        const vis = edgeVisual(e); const eid = 's' + i;
        EDGES.set(eid, e);
        els.push({ group: 'edges', classes: `${vis.cls} seqmsg${self ? ' curved' : ''}`, data: { id: eid, source: a, target: b, cpd: self ? -45 : 0, color: vis.color, line: vis.line, width: vis.width, arrow: vis.arrow, label: vis.label } });
      });
      return { els, pos, rows: rows.length, parts: parts.length };
    }
    function spiralPositions(ids, bandOf, seqIndex, rootId) {
      const pos = new Map();
      const rest = ids.filter((id) => id !== rootId).sort((a, c) => bandOf(a) - bandOf(c) || (seqIndex.get(a) ?? 1e9) - (seqIndex.get(c) ?? 1e9) || a.localeCompare(c));
      if (rootId && ids.includes(rootId)) pos.set(rootId, { x: 0, y: 0 });
      const r0 = 210; const lapGrowth = 200; const arc = 190; let theta = -Math.PI / 2;
      rest.forEach((id) => { const r = r0 + (lapGrowth * (theta + Math.PI / 2)) / TAU; pos.set(id, { x: r * Math.cos(theta), y: r * Math.sin(theta) }); theta += arc / r; });
      return pos;
    }

    // ------------------------------------------------------------------ cytoscape
    const cy = cytoscape({
      container: $('cy'), minZoom: 0.03, maxZoom: 3, boxSelectionEnabled: false, selectionType: 'single',
      userZoomingEnabled: false, // replaced by the wheel listener below (a trackpad swipe pans, a pinch zooms)
      style: [
        { selector: 'node', style: {
          'background-color': 'data(color)', 'border-color': 'data(border)', 'border-width': 1.5, 'border-opacity': 0.9,
          shape: 'data(shape)', width: 'data(w)', height: 'data(h)', label: 'data(label)',
          'font-size': 'data(font)', 'font-family': 'ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif',
          color: '#d8dce3', 'text-valign': 'bottom', 'text-halign': 'center', 'text-margin-y': 4, 'text-wrap': 'wrap', 'text-max-width': '130px',
          'text-background-color': '#14161a', 'text-background-opacity': 0.8, 'text-background-padding': '1px', 'text-background-shape': 'roundrectangle', 'min-zoomed-font-size': 5 } },
        { selector: 'node.context, node.project, node.actor, node.usecase, node.philosophy', style: { 'font-weight': 600 } },
        { selector: 'node.connection', style: { color: '#a5f3fc', 'border-width': 2, 'font-weight': 600 } },
        { selector: 'node:parent', style: {
          'background-opacity': 0.07, 'border-width': 1.5, 'border-opacity': 0.75, 'border-color': 'data(color)', shape: 'round-rectangle',
          'text-valign': 'top', 'text-halign': 'center', 'text-margin-y': -4, 'font-weight': 700, color: 'data(border)', padding: '18px', 'text-background-opacity': 0 } },
        // a box holding boxes: room for the inner box's title (drawn above the inner box) inside the outer border
        { selector: 'node.nest:parent', style: { padding: '30px' } },
        { selector: 'node.context:parent', style: { 'font-size': 26, 'text-max-width': '600px' } },
        { selector: 'node.container:parent', style: { 'font-size': 16, 'text-max-width': '300px' } },
        { selector: 'node.fs-pending', style: { 'border-style': 'dashed', 'border-color': '#e0b341', 'border-width': 2.5, 'border-opacity': 1 } },
        { selector: 'node.fs-designed', style: { 'border-style': 'dashed', 'border-color': '#4da3ff', 'border-width': 2, 'background-opacity': 0.55, 'border-opacity': 1 } },
        { selector: 'node.fs-designed:parent', style: { 'background-opacity': 0.05 } },
        { selector: 'node.fs-delete', style: { 'border-style': 'dashed', 'border-color': '#e05252', opacity: 0.45 } },
        { selector: 'node.test-pass', style: { 'border-color': '#4ade80', 'border-width': 3 } },
        { selector: 'node.test-fail', style: { 'border-color': '#f87171', 'border-width': 3, 'background-color': '#7f1d1d' } },
        { selector: 'node.root', style: { 'border-width': 4, 'border-color': '#f59e0b' } },
        // requirement sections: small pills in a column beside their file, label to the right
        { selector: 'node.req', style: { 'border-width': 1.2, 'text-halign': 'right', 'text-valign': 'center', 'text-margin-x': 4, 'text-margin-y': 0, color: 'data(tcolor)',
          'font-weight': 500, 'text-wrap': 'ellipsis', 'text-max-width': '150px', 'text-background-opacity': 0.55, 'text-background-padding': '1px', 'min-zoomed-font-size': 6 } },
        { selector: 'node.req.rs-draft', style: { 'background-opacity': 0.2, 'border-style': 'dashed' } },
        { selector: 'node.req.rs-done', style: { 'border-color': '#4ade80', 'border-width': 1.6 } },
        { selector: 'edge', style: {
          width: 'data(width)', 'line-color': 'data(color)', 'line-style': 'data(line)', 'target-arrow-color': 'data(color)', 'target-arrow-shape': 'data(arrow)',
          'arrow-scale': 0.85, 'curve-style': 'bezier', opacity: 0.7 } },
        { selector: 'edge.k-codenodes', style: { opacity: 0.55, 'arrow-scale': 0.6 } },
        { selector: 'edge.k-reqs, edge.k-tests', style: { opacity: 0.55 } },
        { selector: 'edge.k-supplies, edge.k-connects', style: { opacity: 0.9, 'arrow-scale': 1 } },
        { selector: 'edge.k-contains', style: { 'curve-style': 'taxi', 'taxi-direction': 'rightward', 'taxi-turn': '-14px', 'taxi-turn-min-distance': '4px', 'target-arrow-shape': 'none', opacity: 0.5 } },
        { selector: 'edge.bad', style: { 'line-dash-pattern': [7, 4] } },
        { selector: 'edge.flow, edge.start', style: {
          opacity: 0.95, label: 'data(label)', 'font-size': 14, 'font-weight': 700, color: 'data(color)', 'text-background-color': '#14161a', 'text-background-opacity': 0.92,
          'text-background-padding': '2px', 'text-background-shape': 'roundrectangle', 'text-border-width': 1, 'text-border-color': 'data(color)', 'text-border-opacity': 0.6, 'z-index': 5 } },
        { selector: 'edge.curved', style: { 'curve-style': 'unbundled-bezier', 'control-point-distances': 'data(cpd)', 'control-point-weights': 0.5 } },
        { selector: 'node.focus', style: { 'font-size': 15, 'text-max-width': '180px' } },
        { selector: 'edge.flow.branch', style: { width: 2.2, opacity: 0.8 } },
        { selector: 'node.anchor', style: { width: 1, height: 1, opacity: 0, label: '', events: 'no' } },
        { selector: 'edge.lifeline', style: { width: 1.2, 'line-color': '#3a414d', 'line-style': 'dashed', 'target-arrow-shape': 'none', opacity: 1, events: 'no' } },
        { selector: 'node.seqhead', style: { 'font-size': 19, 'text-max-width': '172px', 'text-valign': 'top', 'text-margin-y': -4, 'z-index': 20, 'text-background-color': '#14161a', 'text-background-opacity': 0.92, 'text-background-padding': '3px' } },
        { selector: 'edge.seqmsg', style: { 'font-size': 17, 'curve-style': 'straight', 'arrow-scale': 1 } },
        { selector: 'edge.seqmsg.curved', style: { 'curve-style': 'unbundled-bezier' } },
        { selector: 'node.section', style: { width: 1, height: 1, opacity: 1, 'background-opacity': 0, 'border-width': 0, label: 'data(label)', 'font-size': 16, 'font-weight': 700, color: '#8b93a1', 'text-valign': 'center', 'text-halign': 'center', events: 'no' } },
        { selector: '.dim', style: { opacity: 0.12 } },
        { selector: 'node.dim', style: { 'text-opacity': 0.35 } },
        { selector: '.faded', style: { opacity: 0.1 } },
        { selector: '.selfade', style: { opacity: 0.16 } },
        { selector: 'edge.hl', style: { opacity: 1, width: 3, 'z-index': 20 } },
        { selector: 'node.hl', style: { 'z-index': 20 } },
        { selector: 'node.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.45, 'overlay-padding': 7 } },
        { selector: 'edge.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.4, 'overlay-padding': 5 } },
        { selector: 'node.ghost', style: { 'border-style': 'dashed', 'background-opacity': 0.02 } },
        { selector: 'node.picked', style: { 'border-width': 4, 'border-color': '#f8fafc', 'overlay-color': '#38bdf8', 'overlay-opacity': 0.3, 'overlay-padding': 6 } },
        { selector: 'node:selected', style: { 'border-width': 4, 'border-color': '#f8fafc', 'border-style': 'solid', 'overlay-color': '#4da3ff', 'overlay-opacity': 0.18, 'overlay-padding': 6 } },
        { selector: 'edge:selected', style: { width: 4, opacity: 1, 'z-index': 30, 'overlay-color': '#4da3ff', 'overlay-opacity': 0.15, 'overlay-padding': 4 } },
        { selector: 'node.g-flash', style: { 'overlay-color': '#4ade80', 'overlay-opacity': 0.45, 'overlay-padding': 10 } },
      ],
    });
    const isHelper = (el) => el && (el.hasClass('kit-ep') || el.hasClass('anchor') || el.id().startsWith('__'));

    let VIEW = null;
    let hintTimer = null;
    function showBanner(lasting, hint) {
      const b = $('banner');
      if (hintTimer) { clearTimeout(hintTimer); hintTimer = null; }
      const text = [lasting, hint].filter(Boolean).join(' ');
      b.textContent = text; b.classList.toggle('hidden', !text);
      if (!hint) return;
      const clear = () => { if (hintTimer) { clearTimeout(hintTimer); hintTimer = null; } cy.off('pan zoom', onMove); b.textContent = lasting; b.classList.toggle('hidden', !lasting); };
      let armed = false; const onMove = () => { if (armed) clear(); };
      setTimeout(() => { armed = true; }, 400);
      cy.on('pan zoom', onMove); hintTimer = setTimeout(clear, 6000);
    }
    let SEQ_PINNED = false; let SEQ_STATS = null; let LASTING = '';
    function seqStart() {
      const s = SEQ_STATS; cy.fit(undefined, 40); let hint = '';
      if (cy.zoom() < 0.6) { cy.zoom(0.6); const bb = cy.elements().boundingBox(); cy.pan({ x: 40 - bb.x1 * 0.6, y: 30 - bb.y1 * 0.6 }); hint = s ? `${s.rows} steps across ${s.parts} parts: drag or scroll to see the rest, or press Fit.` : ''; } else if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); }
      pinHeads(); return hint;
    }
    function pinHeads() {
      if (!SEQ_PINNED) return;
      const top = -cy.pan().y / cy.zoom();
      cy.nodes('.seqhead').forEach((n) => { const above = n.position('y') - n.boundingBox({ includeLabels: true }).y1; n.position({ x: n.position('x'), y: Math.max(0, top + 8 / cy.zoom() + above) }); });
    }
    let EDGES = new Map(); // cy edge id -> the record it draws ({type:'link'|'flow'|'start', …})
    let LAST_LAYOUT = null;

    /** Positions and viewport of the drawing now, so a redraw can keep them. */
    function snapshot() {
      const pos = new Map();
      cy.nodes().forEach((n) => { if (!isHelper(n) && !n.isParent()) pos.set(n.id(), Object.assign({}, n.position())); });
      return { pos, zoom: cy.zoom(), pan: Object.assign({}, cy.pan()), layout: LAST_LAYOUT, uc: state.uc };
    }

    function render(opts) {
      const o = opts || {};
      writeHash(); syncControls();
      $('showhidden').classList.toggle('hidden', !HIDDEN.size);
      $('showhidden').textContent = HIDDEN.size ? `Show hidden (${HIDDEN.size})` : 'Show hidden';
      hideTip();
      const v = currentView(); VIEW = v;
      let layout = state.layout;
      const banners = [];
      if (layout === 'sequence' && (!v.uc || state.dim || !v.uc.has_flowmap)) {
        if (v.uc) banners.push(state.dim ? 'Sequence needs the use case on its own: untick "dim the rest". Showing Tiered.' : 'Sequence draws numbered steps, and this use case has no flowmap yet. Showing Tiered.');
        layout = 'tiered';
      }
      if (v.uc && !v.uc.has_flowmap) banners.push('This use case has no flowmap yet: showing the actors that drive it and the parts it uses.');
      if (v.uc && v.uc.unresolved.length) banners.push(`${v.uc.unresolved.length} flowmap reference(s) match no node: ${v.uc.unresolved.slice(0, 6).join(', ')}${v.uc.unresolved.length > 6 ? '…' : ''}`);
      EDGES = new Map();
      SEQ_PINNED = layout === 'sequence';
      let hint = '';
      if (layout === 'sequence') {
        const s = sequenceElements(v);
        cy.batch(() => { cy.elements().remove(); cy.add(s.els); });
        cy.resize(); cy.nodes().positions((n) => s.pos.get(n.id()) || { x: 0, y: 0 });
        SEQ_STATS = s; hint = seqStart();
        LASTING = banners.join(' '); showBanner(LASTING, hint);
        $('empty').classList.toggle('hidden', cy.nodes().nonempty());
        LAST_LAYOUT = layout;
        renderUcSummary(v); applySearch(false); if (SELECTED) reselect();
        return;
      }
      LASTING = banners.join(' '); showBanner(LASTING, hint);
      const all = new Set(v.ids); const ghosts = new Set();
      if (layout === 'cluster' && v.uc && !state.dim) {
        for (const id of v.ids) { let p = (nodesById.get(id) || {}).parent; while (p) { if (!all.has(p)) ghosts.add(p); p = (nodesById.get(p) || {}).parent; } }
        ghosts.forEach((g) => all.add(g));
      }
      const parentFor = (id) => {
        if (layout !== 'cluster') return undefined;
        let p = (nodesById.get(id) || {}).parent;
        while (p) { if (all.has(p)) return p; p = (nodesById.get(p) || {}).parent; }
        return undefined;
      };
      const ub = v.uc && !state.dim ? ucBands(v) : null;
      const bands = ub ? ub.bands : null;
      const els = [];
      for (const id of all) {
        const n = nodesById.get(id); if (!n) continue;
        const classes = nodeClasses(n);
        if (v.uc && state.dim && !v.touched.has(id)) classes.push('dim');
        if (ghosts.has(id)) classes.push('ghost');
        if (v.uc && id === v.uc.id) classes.push('root');
        if (!v.whole) classes.push('focus');
        const st = LEVELS[n.level] || LEVELS.component; const big = v.uc && id === v.uc.id;
        els.push({ group: 'nodes', position: { x: Math.random() * 800, y: Math.random() * 600 }, classes: classes.join(' '), data: nodeData(n, { w: big ? st.w * 1.3 : st.w, h: big ? st.h * 1.3 : st.h, parent: parentFor(id) }) });
      }
      const rowRank = new Map();
      if (bands) { const vals = [...new Set([...all].map((id) => bands.get(id)))].sort((a, b) => a - b); all.forEach((id) => rowRank.set(id, vals.indexOf(bands.get(id)))); }
      // in Cluster, the box already says "owns": that line is drawn only on request
      const list = v.edges.filter((e) => all.has(e.source) && all.has(e.target)
        && !(e.type === 'link' && e.kind === 'codenodes' && layout === 'cluster' && !state.own && parentFor(e.target) === e.source));
      const bow = list.map((m) => {
        if (!(bands && layout === 'tiered' && m.type === 'flow')) return 0;
        const a = rowRank.get(m.source); const b = rowRank.get(m.target); const span = Math.abs(b - a);
        if (a === b) return -50; if (b < a) return 220 + 40 * span; if (span > 1) return 200 + 40 * span; return 0;
      });
      const cpds = spreadParallel(list, bow);
      list.forEach((m, i) => {
        const vis = edgeVisual(m); const eid = m.type === 'link' ? 'e:' + m.link.id : 'f:' + m.id;
        EDGES.set(eid, m);
        const classes = [vis.cls];
        if (v.uc && state.dim && m.type !== 'flow' && m.type !== 'start') classes.push('dim');
        if (cpds[i] !== null) classes.push('curved');
        els.push({ group: 'edges', classes: classes.join(' '), data: { id: eid, source: m.source, target: m.target, cpd: cpds[i] || 0, color: vis.color, line: vis.line, width: vis.width, arrow: vis.arrow, label: vis.label,
          kind: m.kind || '', from: m.type === 'link' ? m.link.from : '', to: m.type === 'link' ? m.link.to : '' } });
      });
      cy.batch(() => { cy.elements().remove(); cy.add(els); cy.nodes(':parent').forEach((p) => { p.toggleClass('nest', p.children(':parent').nonempty()); }); });
      $('empty').classList.toggle('hidden', cy.nodes().nonempty());
      $('empty').innerHTML = HIDE_TYPES.size && G.nodes.length ? 'Nothing to show: every node type on this project is switched off above. <a href="#" id="g-showall">Show all types</a>' : 'Nothing to show with these settings.';
      const sa = $('showall'); if (sa) sa.onclick = (e) => { e.preventDefault(); HIDE_TYPES.clear(); render(); };
      renderUcSummary(v);
      // a redraw after an edit keeps the picture: known nodes stay put, new ones sit beside a neighbour
      const keep = o.keep && o.keep.layout === layout && o.keep.uc === state.uc ? o.keep : null;
      let placed = 0; let total = 0;
      if (keep) {
        const leaf = cy.nodes().filter((n) => !n.isParent());
        total = leaf.length;
        leaf.forEach((n) => { const p = keep.pos.get(n.id()); if (p) { n.position(p); placed++; } });
      }
      if (keep && placed && placed >= total * 0.6) {
        // a new node goes beside a neighbour that kept its place, in the first free spot on a spiral around it
        const taken = []; cy.nodes().filter((n) => !n.isParent() && keep.pos.has(n.id())).forEach((n) => taken.push(n.position()));
        const free = (p) => taken.every((q) => Math.abs(q.x - p.x) > 90 || Math.abs(q.y - p.y) > 60);
        // new requirement sections line up beside their file instead
        const newReqs = new Set(cy.nodes('.req').filter((n) => !keep.pos.has(n.id())).map((n) => n.id()));
        cy.nodes().filter((n) => !n.isParent() && !keep.pos.has(n.id()) && !newReqs.has(n.id())).forEach((n) => {
          const nb = n.neighborhood('node').filter((x) => keep.pos.has(x.id()) && !x.isParent())[0] || (n.parent().nonempty() ? n.parent().children().filter((x) => keep.pos.has(x.id()))[0] : null);
          const at = nb ? nb.position() : (taken[0] || { x: 0, y: 0 });
          let p = { x: at.x + 110, y: at.y };
          for (let i = 1; i < 80 && !free(p); i++) { const a = i * 0.9; const r = 90 + i * 14; p = { x: at.x + r * Math.cos(a), y: at.y + r * Math.sin(a) }; }
          n.position(p); taken.push(p);
        });
        if (newReqs.size) placeReqs(newReqs);
        cy.resize(); cy.viewport({ zoom: keep.zoom, pan: keep.pan });
      } else {
        runLayout(layout, v, ub);
      }
      LAST_LAYOUT = layout;
      applySearch(false);
      if (SELECTED) reselect();
    }

    function runLayout(layout, v, ub) {
      const bands = ub ? ub.bands : null;
      cy.resize();
      if (cy.nodes().empty()) return;
      // requirement sections are placed beside their file afterwards (placeReqs); the position maths leave them out
      const reqDrawn = cy.nodes('.req').nonempty();
      const ids = cy.nodes().filter((n) => !n.hasClass('req')).map((n) => n.id());
      const layEdges = cy.edges().filter((e) => !e.hasClass('k-contains')).map((e) => ({ source: e.source().id(), target: e.target().id() }));
      const inView = new Set(ids);
      const structural = G.edges.filter((e) => e.kind === 'codenodes' && inView.has(e.from) && inView.has(e.to)).map((e) => ({ source: e.from, target: e.to }));
      const wholeBandT = (id) => (id === (v.uc && v.uc.id) ? -1 : (WHOLE_TIER[levelOf(id)] ?? 8));
      const wholeBandR = (id) => (id === (v.uc && v.uc.id) ? -1 : (WHOLE_RING[levelOf(id)] ?? 7));
      const bandOf = bands ? (id) => bands.get(id) ?? 99 : null;
      let pos = null;
      const aspect = Math.min(3, Math.max(0.8, cy.width() / Math.max(1, cy.height())));
      if (layout === 'cluster') {
        cy.layout({ name: 'fcose', quality: 'default', randomize: true, animate: false, nodeDimensionsIncludeLabels: true,
          idealEdgeLength: () => 85, nodeRepulsion: () => 11000, nodeSeparation: 90, edgeElasticity: () => 0.15, nestingFactor: 0.1, gravity: 0.5, gravityCompound: 2, gravityRangeCompound: 1.0,
          numIter: 4000, packComponents: true, ...(window.__fcoseOverride || {}), tile: true, tilingPaddingVertical: 20, tilingPaddingHorizontal: 20, fit: false }).run();
      } else if (layout === 'flow') {
        cy.layout({ name: 'dagre', rankDir: 'LR', nodeSep: 26, rankSep: 110, edgeSep: 8, ranker: 'network-simplex', nodeDimensionsIncludeLabels: true, fit: false, animate: false }).run();
      } else if (layout === 'tiered') {
        pos = tieredPositions(ids, layEdges.concat(structural), bandOf || wholeBandT, bands ? { tierGap: 200, colGap: 210, aspect } : { aspect, colGap: reqDrawn ? 250 : 150 });
      } else if (layout === 'rings') {
        if (bands) pos = spiralPositions(ids, bandOf, new Map(v.uc.sequence.map((id, i) => [id, i])), v.uc.id);
        else pos = ringPositions(ids, layEdges.concat(structural), wholeBandR, { maxSubRings: 2, minArc: reqDrawn ? 210 : 120, ringGap: reqDrawn ? 230 : 190, subRingGap: 95 });
      }
      if (pos) cy.nodes().filter((n) => !n.hasClass('req')).positions((n) => pos.get(n.id()) || { x: 0, y: 0 });
      if (reqDrawn) placeReqs(null);
      cy.fit(undefined, v.uc && !state.dim ? 55 : 30);
      if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); }
    }

    /**
     * Requirement sections sit in a column just right of their file (in file order), joined by short
     * taxi lines, like a table of contents. only = a Set of req ids: re-place just the columns holding one.
     */
    const REQ_COL = { dx: 30, dy: 15 };
    function placeReqs(only) {
      const cols = new Map();
      cy.nodes('.req').forEach((el) => { const n = nodesById.get(el.id()); if (!n) return; if (!cols.has(n.file)) cols.set(n.file, []); cols.get(n.file).push(n); });
      cols.forEach((list, fid) => {
        if (only && !list.some((n) => only.has(n.id))) return;
        const f = cy.getElementById(fid);
        list.sort((a, b) => a.order - b.order);
        let x0; let y0;
        if (f.nonempty() && !f.isParent()) {
          // right of the file and clear of its name (drawn under it), centred on it
          const p = f.position(); let lab = p.x;
          try { lab = f.boundingBox({ includeLabels: true, includeNodes: false, includeOverlays: false }).x2; } catch (e) { /* no label box yet */ }
          x0 = Math.max(p.x + f.width() / 2 + REQ_COL.dx, (Number.isFinite(lab) ? lab : p.x) + 12); y0 = p.y - ((list.length - 1) * REQ_COL.dy) / 2;
        }
        else { // the file is not drawn: keep the column where the first section already is
          const p = cy.getElementById(list[0].id).position(); x0 = p.x; y0 = p.y;
        }
        list.forEach((n, i) => cy.getElementById(n.id).position({ x: x0, y: y0 + i * REQ_COL.dy }));
      });
    }
    // dragging a file takes its column of sections along
    let fileDrag = null;
    cy.on('grab', 'node.bizreq, node.techreq', (evt) => { fileDrag = { id: evt.target.id(), at: Object.assign({}, evt.target.position()) }; });
    cy.on('drag', 'node.bizreq, node.techreq', (evt) => {
      if (!fileDrag || fileDrag.id !== evt.target.id()) return;
      const p = evt.target.position(); const dx = p.x - fileDrag.at.x; const dy = p.y - fileDrag.at.y;
      fileDrag.at = Object.assign({}, p);
      cy.nodes('.req').forEach((el) => { const n = nodesById.get(el.id()); if (n && n.file === fileDrag.id) el.position({ x: el.position('x') + dx, y: el.position('y') + dy }); });
    });
    cy.on('free', 'node.bizreq, node.techreq', () => { fileDrag = null; });

    function renderUcSummary(v) {
      const box = $('ucsummary');
      if (!v.uc) { box.classList.add('hidden'); return; }
      const u = v.uc;
      const counts = u.has_flowmap ? `${u.process_steps.length} process step(s), ${u.data_steps.length} data step(s), ${u.sequence.length} parts` : `${u.uses.length} part(s) used, ${u.actors.length} actor(s) (no flowmap yet)`;
      box.innerHTML = `<h2 title="Click to fold or unfold">${esc(u.name)} <span class="fold">▾</span></h2><div class="body"><p>${esc(u.summary || u.desc)}</p><p class="muted">${esc(counts)}</p></div>`;
      box.querySelector('h2').onclick = () => box.classList.toggle('folded');
      box.classList.remove('hidden');
    }

    // ------------------------------------------------------------------ hover
    let tipTimer = null;
    function hideTip() { if (tipTimer) clearTimeout(tipTimer); tipTimer = null; $('tip').classList.add('hidden'); }
    cy.on('mouseover', 'node', (evt) => {
      if (isHelper(evt.target)) return;
      if (!SELECTED && !(window.IterGraphEdit && IterGraphEdit.drawing)) {
        const hood = evt.target.closedNeighborhood().union(evt.target.ancestors()).union(evt.target.descendants()).union(stepEdgesOf(evt.target.id()));
        cy.elements().not(hood).not('.kit-ep').addClass('faded'); hood.addClass('hl');
      }
      const n = nodesById.get(evt.target.id()); if (!n) return;
      const p = evt.renderedPosition || evt.target.renderedPosition();
      if (tipTimer) clearTimeout(tipTimer);
      tipTimer = setTimeout(() => {
        const fs = FILE_STATES[n.file_state];
        const tip = $('tip');
        if (n.nodetype === 'req') {
          tip.innerHTML = `<div><b>${esc(n.name)}</b> <span class="sub">${esc(n.status || 'draft')} · in ${esc(nameOf(n.file))}</span></div>${n.text ? `<div class="sub">${esc(trunc(n.text.replace(/\s+/g, ' '), 220))}</div>` : ''}`;
          tip.style.left = p.x + 'px'; tip.style.top = p.y + 'px'; tip.classList.remove('hidden');
          return;
        }
        tip.innerHTML = `<div><b>${esc(n.name)}</b> <span class="sub">${esc((LEVELS[n.level] || {}).label || n.level)}${n.file_state !== 'synced' ? ' · ' + esc(fs.label) : ''}</span></div>${n.desc ? `<div class="sub">${esc(trunc(n.desc, 220))}</div>` : ''}`;
        tip.style.left = p.x + 'px'; tip.style.top = p.y + 'px'; tip.classList.remove('hidden');
      }, 450);
    });
    cy.on('mouseout', 'node', () => { cy.elements().removeClass('faded hl'); hideTip(); });
    cy.on('mouseover', 'edge', (evt) => {
      const e = evt.target; if (isHelper(e)) return;
      if (!SELECTED) { cy.elements().not(e.union(e.connectedNodes())).not('.kit-ep').addClass('faded'); e.addClass('hl'); }
      const p = evt.renderedPosition;
      if (tipTimer) clearTimeout(tipTimer);
      tipTimer = setTimeout(() => {
        const m = EDGES.get(e.id()); if (!m) return;
        const tip = $('tip');
        tip.innerHTML = `<div><b>${esc(edgeTitle(m))}</b></div><div class="sub">${esc(nameOf(m.source))} → ${esc(nameOf(m.target))}</div>`;
        tip.style.left = p.x + 'px'; tip.style.top = p.y + 'px'; tip.classList.remove('hidden');
      }, 450);
    });
    cy.on('mouseout', 'edge', () => { cy.elements().removeClass('faded hl'); hideTip(); });
    cy.on('pan zoom tapstart', hideTip);
    cy.on('pan zoom', () => { if (!SELECTED) cy.elements('.faded, .hl').removeClass('faded hl'); });
    cy.on('viewport', pinHeads);
    const nameOf = (id) => (nodesById.get(id) || {}).name || id;
    const trunc = (s, n) => { s = String(s || ''); return s.length > n ? s.slice(0, n - 1) + '…' : s; };
    const stepText = (e) => (e.kind === 'process' ? e.what : e.data) || e.what || e.data || '';
    function edgeTitle(m) {
      if (m.type === 'flow') return `Step ${stepName(m)} (${m.kind}${m.branch ? ', branch ' + m.branch : ''}): ${m.plain || stepText(m)}`;
      if (m.type === 'start') return 'The use case starts here';
      return `${(EDGE_KINDS[m.kind] || {}).label || m.kind}`;
    }

    // ------------------------------------------------------------------ selection + detail panel
    let SELECTED = null; // {kind:'node'|'edge', id, key?}
    cy.on('tap', 'node', (evt) => { if (isHelper(evt.target) || (window.IterGraphEdit && IterGraphEdit.drawing)) return; select({ kind: 'node', id: evt.target.id() }); });
    cy.on('tap', 'edge', (evt) => { if (isHelper(evt.target)) return; select({ kind: 'edge', id: evt.target.id() }); });
    cy.on('tap', (evt) => { if (evt.target === cy) select(null); });

    function select(sel, opts) {
      SELECTED = sel;
      cy.elements().removeClass('selfade faded hl picked');
      if (!sel) { cy.elements(':selected').unselect(); $('detail').classList.add('hidden'); cy.resize(); emitSel(); return; }
      reselect();
      if (!SELECTED) { $('detail').classList.add('hidden'); cy.resize(); emitSel(); return; }
      $('detail').classList.remove('hidden');
      cy.resize();
      if (!(opts && opts.noPan)) keepInView(cy.getElementById(SELECTED.id));
      renderDetail();
      emitSel();
    }
    /** The detail pane takes the right of the canvas: slide the drawing so the selection is not left under it or off screen. */
    function keepInView(el) {
      if (!el || el.empty()) return;
      const p = el.isEdge() ? el.renderedMidpoint() : el.renderedPosition();
      if (!p || !Number.isFinite(p.x)) return;
      const w = cy.width(); const h = cy.height(); const m = 60;
      let dx = p.x < m ? m - p.x : p.x > w - m ? (w - m) - p.x : 0;
      if (el.isNode() && el.hasClass('req')) { const bb = el.renderedBoundingBox({ includeLabels: true }); if (bb.x2 + dx > w - 16) dx = (w - 16) - bb.x2; }
      const dy = p.y < m ? m - p.y : p.y > h - m ? (h - m) - p.y : 0;
      if (dx || dy) cy.animate({ panBy: { x: dx, y: dy } }, { duration: 200 });
    }
    function emitSel() { ROOT.dispatchEvent(new CustomEvent('g-select', { detail: SELECTED ? Object.assign({}, SELECTED) : null })); }
    function reselect() {
      const el = cy.getElementById(SELECTED.id);
      if (!el || el.empty()) { if (SELECTED.kind === 'edge') SELECTED = null; return; }
      cy.elements(':selected').not(el).unselect();
      if (!el.selected()) el.select();
      let keep;
      if (el.isNode()) keep = el.closedNeighborhood().union(el.ancestors()).union(stepEdgesOf(el.id()));
      else {
        const m = EDGES.get(el.id());
        const ends = m ? cy.getElementById(m.source).union(cy.getElementById(m.target)) : cy.collection();
        ends.addClass('picked');
        keep = el.union(el.connectedNodes()).union(ends).union(ends.ancestors());
      }
      cy.elements().not(keep).not('.kit-ep').addClass('selfade');
    }
    function stepEdgesOf(id) {
      const es = cy.edges('.seqmsg').filter((ed) => { const m = EDGES.get(ed.id()); return !!m && (m.source === id || m.target === id); });
      return es.union(es.connectedNodes()).union(cy.getElementById('lle|' + id));
    }
    function focusNode(id, opts) {
      const rq = nodesById.get(id);
      if (rq && rq.nodetype === 'req' && !typeShown(rq) && nodesById.has(rq.file)) {
        // sections are not drawn: open the file's table at that row instead
        EXPANDED.add(id);
        const ok = focusNode(rq.file, opts);
        const tr = $('detail').querySelector(`tr.g-rrow[data-req="${CSS.escape(id)}"]`);
        if (tr) { tr.scrollIntoView({ block: 'nearest' }); tr.classList.add('g-rflash'); setTimeout(() => tr.classList.remove('g-rflash'), 1600); }
        return ok;
      }
      if (HIDDEN.has(id)) { HIDDEN.delete(id); saveHidden(); render({ keep: snapshot() }); }
      let el = cy.getElementById(id);
      const n = nodesById.get(id);
      if (el.empty() && n && HIDE_TYPES.has(n.level)) { HIDE_TYPES.delete(n.level); render({ keep: snapshot() }); el = cy.getElementById(id); }
      if (el.empty() && state.uc) { setUc(''); render(); el = cy.getElementById(id); }
      if (el.empty()) return false;
      select({ kind: 'node', id }, { noPan: true });
      cy.animate({ center: { eles: el }, zoom: Math.max(cy.zoom(), 0.9) }, { duration: 250 });
      if (opts && opts.flash) { el.addClass('g-flash'); setTimeout(() => el.removeClass('g-flash'), 1600); }
      return true;
    }

    const dot = (id) => `<span class="dot" style="background:${(LEVELS[levelOf(id)] || LEVELS.component).color}"></span>`;
    const nodeLink = (id) => `${dot(id)}<a class="node" data-node="${esc(id)}">${esc(nameOf(id))}</a>`;
    const STATUS_TEXT = { 'not-built': 'not built yet', broken: 'not working yet', 'not-deployed': 'not deployed yet', live: 'working', complete: 'complete' };
    const statusTag = (s) => (s ? `<span class="st ${isBad(s) ? 'bad' : 'ok'}">${esc(STATUS_TEXT[String(s).toLowerCase()] || s)}</span>` : '');
    const evidenceHtml = (ev) => { if (!ev) return ''; const l = Array.isArray(ev) ? ev : [ev]; return `<div class="muted">Evidence: ${l.map((x) => `<code>${esc(x)}</code>`).join(', ')}</div>`; };
    const fmtTs = (t) => { if (!t) return '—'; const d = new Date(String(t).replace(' ', 'T')); return isNaN(d) ? String(t) : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }); };
    const fsBadge = (n) => `<span class="g-fs g-fs-${esc(n.file_state)}" title="${esc({ synced: 'the file in the repository matches this node', pending_write: 'changed here; waiting for an engine to write the file', pending_delete: 'deleted here; waiting for an engine to remove the file', designed: 'exists only in the designer: no repository yet (Build creates it)' }[n.file_state] || '')}">${esc(FILE_STATES[n.file_state].label)}</span>`;

    function renderDetail() {
      const box = $('detail');
      let html = '<button class="close" title="Close (Esc)" aria-label="Close">×</button>';
      html += SELECTED.kind === 'node' ? nodeDetail(SELECTED.id) : edgeDetail(EDGES.get(SELECTED.id));
      box.innerHTML = html; box.scrollTop = 0;
      const sn = SELECTED.kind === 'node' ? nodesById.get(SELECTED.id) : null;
      const wide = isReqFile(sn);
      if (box.classList.contains('g-wide') !== wide) { box.classList.toggle('g-wide', wide); cy.resize(); }
      wireReqTable(box);
      box.querySelector('.close').onclick = () => select(null);
      box.querySelectorAll('button.hidenode').forEach((b) => { b.onclick = () => hideNode(b.dataset.hide); });
      box.querySelectorAll('a.node').forEach((a) => { a.onclick = () => focusNode(a.dataset.node); });
      box.querySelectorAll('a.uc').forEach((a) => { a.onclick = () => { setUc(a.dataset.uc); render(); }; });
      box.querySelectorAll('a.edge').forEach((a) => { a.onclick = () => focusEdgeId(a.dataset.edge); });
      ROOT.dispatchEvent(new CustomEvent('g-detail', { detail: { kind: SELECTED.kind, id: SELECTED.id, node: SELECTED.kind === 'node' ? nodesById.get(SELECTED.id) : null, edge: SELECTED.kind === 'edge' ? EDGES.get(SELECTED.id) : null, box } }));
    }

    function testHtml(t) {
      if (!t || typeof t !== 'object') return '<p class="muted">Not run yet.</p>';
      const ok = t.overall_success != null ? t.overall_success : (t.result ? t.result === 'green' : null);
      const bucket = (k) => { const b = t[k]; if (!b || typeof b !== 'object') return ''; return `<div class="g-tb"><span>${esc(k)}</span><b>${esc(b.pass ?? 0)}/${esc(b.total ?? 0)}</b>${b.err ? `<em>${esc(b.err)} err</em>` : ''}</div>`; };
      let h = `<div class="g-test ${ok === true ? 'ok' : ok === false ? 'bad' : ''}"><b>${ok === true ? '✓ passing' : ok === false ? '✗ failing' : 'result'}</b>${t.ts || t.at ? ` <span class="muted">${esc(fmtTs(t.ts || t.at))}</span>` : ''}
        <div class="g-tbs">${bucket('normal')}${bucket('longtail')}${bucket('failure')}</div></div>`;
      if (t.counts) h += `<p class="muted">${esc(t.counts)}</p>`;
      const det = Array.isArray(t.details) ? t.details : [];
      if (det.length) {
        const bad = det.filter((d) => d && d.pass === false);
        h += `<details${bad.length ? ' open' : ''}><summary>${det.length} test(s)${bad.length ? `, ${bad.length} failing` : ''}</summary><ul class="g-tlist">${det.slice(0, 200).map((d) => `<li class="${d.pass ? 'ok' : 'bad'}"><span>${d.pass ? '✓' : '✗'}</span> ${esc(d.name || '')} <span class="muted">${esc(d.bucket || '')}</span>${d.msg ? `<div class="muted">${esc(d.msg)}</div>` : ''}</li>`).join('')}</ul></details>`;
      }
      return h;
    }
    function edgeList(title, list, otherOf, verb) {
      if (!list.length) return '';
      const byKind = new Map(); list.forEach((e) => { if (!byKind.has(e.kind)) byKind.set(e.kind, []); byKind.get(e.kind).push(e); });
      let h = `<h4>${esc(title)}</h4>`;
      byKind.forEach((es, k) => {
        h += `<div class="g-ekind"><span class="g-eswatch" style="background:${(EDGE_KINDS[k] || {}).color || '#888'}"></span>${esc(verb(k))} <span class="muted">(${es.length})</span></div><ul>`
          + es.slice(0, 80).map((e) => `<li>${nodeLink(otherOf(e))} <a class="edge g-elink" data-edge="${esc('e:' + e.id)}" title="select this edge">↗</a></li>`).join('') + (es.length > 80 ? `<li class="muted">… ${es.length - 80} more</li>` : '') + '</ul>';
      });
      return h;
    }
    const VERB_OUT = { codenodes: 'owns', tests: 'tested by', reqs: 'requirements', supplies: 'supplies', connects: 'connects to', drives: 'drives', touches: 'touches', uses: 'uses', contains: 'holds' };
    const VERB_IN = { codenodes: 'owned by', tests: 'tests', reqs: 'required by', supplies: 'supplied by', connects: 'reached through', drives: 'driven by', touches: 'touched by', uses: 'used by', contains: 'in file' };
    function nodeDetail(id) {
      const n = nodesById.get(id); if (!n) return '';
      if (n.nodetype === 'req') return reqDetail(n);
      const st = LEVELS[n.level] || LEVELS.component;
      let h = `<h2>${esc(n.name)}</h2><div class="g-badges"><span class="badge" style="background:${st.color}">${esc(st.label)}</span>${fsBadge(n)}${n.teststate && n.teststate !== 'inherit' ? `<span class="g-pill" title="teststate">tests: ${esc(n.teststate)}</span>` : ''}${n.front.status && !isReqFile(n) ? `<span class="g-pill">${esc(n.front.status)}</span>` : ''}${n.front.owner ? `<span class="g-pill">${esc(n.front.owner)}</span>` : ''}
        <button type="button" class="hidenode" data-hide="${esc(id)}" title="Take this node and its lines off the drawing until Show hidden (this browser tab only)">Hide</button></div>`;
      h += n.desc ? `<div class="simple">${esc(n.desc)}</div>` : '<div class="simple missing">No description yet (desc is empty).</div>';
      h += '<div class="g-actions-slot"></div><div class="g-conflicts-slot"></div>';
      if (isReqFile(n)) h += reqTableHtml(n);
      if (n.nodetype === 'project' || n.nodetype === 'code') h += reqFilesHtml(n);
      if (n.nodetype === 'project') h += '<div class="g-build-slot"></div>';
      if (n.nodetype === 'test') {
        // n.test = the server's summary (outcome, when); front.last_result = the full standard result (buckets, details)
        const lr = n.front && n.front.last_result && typeof n.front.last_result === 'object' ? n.front.last_result : null;
        const t = lr || (n.test && typeof n.test === 'object' && Object.keys(n.test).length) ? Object.assign({}, lr || {}, n.test || {}) : null;
        h += `<h4>Last result</h4>${testHtml(t)}<div class="g-testlogs-slot"></div>`;
      }
      if (n.level === 'connection') {
        const sup = (inE.get(id) || []).filter((e) => e.kind === 'supplies').map((e) => e.from);
        const to = (outE.get(id) || []).filter((e) => e.kind === 'connects').map((e) => e.to);
        h += `<div class="g-conn"><div><h4>Supplied by (${sup.length})</h4>${sup.length ? sup.map((x) => `<div>${nodeLink(x)}</div>`).join('') : '<p class="muted">Nothing supplies it yet.</p>'}</div>
          <div class="g-connarrow">→ ◆ →</div><div><h4>Connects to (${to.length})</h4>${to.length ? to.map((x) => `<div>${nodeLink(x)}</div>`).join('') : '<p class="muted">It reaches nothing yet.</p>'}</div></div>`;
      }
      if (n.body && n.body.trim()) h += `<details class="g-body"${n.body.length < 1600 && !isReqFile(n) ? ' open' : ''}><summary>${isReqFile(n) ? 'File body (markdown)' : 'Body'}</summary>${window.IterKit ? IterKit.md(n.body) : `<pre>${esc(n.body)}</pre>`}</details>`;
      h += `<h4>File</h4><span class="path">${esc(n.path || '(no path yet)')}</span>`;
      h += `<dl class="g-facts"><dt>creator</dt><dd>${esc(n.creator || '—')}</dd><dt>created</dt><dd>${esc(fmtTs(n.timestamps.create))}</dd><dt>modified</dt><dd>${esc(fmtTs(n.timestamps.last_modified))}</dd>${n.nodetype === 'test' || n.timestamps.last_tested ? `<dt>tested</dt><dd>${esc(fmtTs(n.timestamps.last_tested))}</dd>` : ''}${n.node_version != null ? `<dt>version</dt><dd>v${esc(n.node_version)}${n.file_version && n.file_version !== n.node_version ? ` <span class="muted">(file v${esc(n.file_version)})</span>` : ''}</dd>` : ''}<dt>id</dt><dd class="g-id" title="click to copy">${esc(id)}</dd></dl>`;
      const chain = []; let p = n.parent; while (p) { chain.unshift(p); p = (nodesById.get(p) || {}).parent; }
      if (chain.length) h += `<h4>Inside</h4><div class="chain">${chain.map(nodeLink).join('')}</div>`;
      if (n.level === 'usecase') h += usecaseSteps(id);
      const ucs = USECASES.filter((u) => u.id !== id && u.sequence.includes(id));
      if (ucs.length) h += `<h4>Use cases through it (${ucs.length})</h4><ul>${ucs.map((u) => `<li><a class="uc" data-uc="${esc(u.id)}">${esc(u.name)}</a></li>`).join('')}</ul>`;
      if (VIEW && VIEW.uc && n.level !== 'usecase') {
        const mine = (flowsByUc.get(VIEW.uc.id) || []).filter((e) => e.source === id || e.target === id);
        if (mine.length) { h += '<h4>Its steps in this use case</h4>'; mine.sort((a, b) => (a.kind === b.kind ? 0 : a.kind === 'process' ? -1 : 1) || (a.step ?? 0) - (b.step ?? 0)).forEach((e) => { h += flowCard(e); }); }
      }
      h += edgeList('Links out', (outE.get(id) || []).filter((e) => !(n.level === 'connection' && e.kind === 'connects') && e.kind !== 'contains'), (e) => e.to, (k) => VERB_OUT[k] || k);
      h += edgeList('Linked from', (inE.get(id) || []).filter((e) => !(n.level === 'connection' && e.kind === 'supplies')), (e) => e.from, (k) => VERB_IN[k] || k);
      const ch = n.children || {};
      const refs = ['codedirs'].concat(Object.keys(ch).filter((k) => !['codedirs', 'codenodes', 'tests', 'reqs'].includes(k))).filter((k) => Array.isArray(ch[k]) && ch[k].length);
      if (refs.length) h += `<h4>Children (paths and globs)</h4>${refs.map((k) => `<div class="muted">${esc(k)}</div><ul class="codefiles">${ch[k].map((x) => `<li><span class="path">${esc(x)}</span></li>`).join('')}</ul>`).join('')}`;
      const extra = Object.keys(n.front || {}).filter((k) => !['flowmap', 'last_result', 'status', 'owner', 'level', 'connects', 'drives', 'touches', 'actors'].includes(k));
      if (extra.length) h += `<details><summary>More fields (${extra.length})</summary><dl class="g-facts">${extra.map((k) => `<dt>${esc(k)}</dt><dd>${esc(typeof n.front[k] === 'object' ? JSON.stringify(n.front[k]) : n.front[k])}</dd>`).join('')}</dl></details>`;
      return h;
    }
    // ------------------------------------------------------------------ requirements (spec §2.8): the table on a file, the files on a code node, one section
    const EXPANDED = new Set(); // requirement rows open in the table (kept across redraws)
    const md = (t) => (window.IterKit ? IterKit.md(t) : `<pre>${esc(t)}</pre>`);
    const STATUS_TIP = { draft: 'draft: proposed, not yet agreed', agreed: 'agreed: everyone signed up to it', done: 'done: built and verified' };
    const statusPill = (r) => { const st = REQ_STATUS.includes(r.status) ? r.status : 'draft'; return `<span class="g-rst rs-${st}" data-rstatus="${esc(r.id)}" title="${esc(STATUS_TIP[st])}">${esc(r.status || 'draft')}</span>`; };
    function reqRowHtml(r) {
      const open = EXPANDED.has(r.id);
      return `<tr class="g-rrow${open ? ' open' : ''}" data-req="${esc(r.id)}" tabindex="0" aria-expanded="${open}" title="Click to ${open ? 'fold' : 'read'} the requirement">
          <td class="k">${r.key ? esc(r.key) : '<span class="g-nokey">—</span>'}</td><td class="t">${esc(r.title || '(untitled)')}</td><td class="s">${statusPill(r)}</td><td class="x"><span class="g-chev" aria-hidden="true"></span></td></tr>
        <tr class="g-rmore${open ? '' : ' hidden'}" data-req-more="${esc(r.id)}"><td colspan="4"><div class="g-rtext">${r.text ? md(r.text) : '<p class="muted">No text yet.</p>'}</div><div class="g-racts" data-req-acts="${esc(r.id)}"></div></td></tr>`;
    }
    function statusBar(list) {
      if (!list.length) return '';
      const by = {}; list.forEach((r) => { const k = REQ_STATUS.includes(r.status) ? r.status : 'draft'; by[k] = (by[k] || 0) + 1; });
      return `<div class="g-rbar" aria-hidden="true">${REQ_STATUS.filter((k) => by[k]).map((k) => `<span class="rs-${k}" style="flex:${by[k]}"></span>`).join('')}</div>
        <div class="g-rlegend">${REQ_STATUS.filter((k) => by[k]).map((k) => `<span><i class="rs-${k}"></i>${by[k]} ${k}</span>`).join('')}</div>`;
    }
    function reqTableHtml(f) {
      const list = f.reqs || [];
      let h = `<section class="g-reqs ft-${esc(f.nodetype)}" data-file="${esc(f.id)}"><div class="g-reqs-head"><h4>Requirements <span class="g-count">${list.length}</span></h4><span class="g-reqs-tools"></span></div>${statusBar(list)}`;
      if (!list.length) h += '<div class="g-rempty">No requirements in this file yet. Each requirement is one <code>## KEY — title</code> section of the file.</div>';
      else h += `<div class="g-rwrap"><table class="g-rtable"><thead><tr><th class="k">Key</th><th class="t">Title</th><th class="s">Status</th><th class="x"><span class="g-rall" title="Open or fold every requirement" role="button" tabindex="0">${list.every((r) => EXPANDED.has(r.id)) ? 'fold' : 'open'} all</span></th></tr></thead><tbody>${list.map(reqRowHtml).join('')}</tbody></table></div>`;
      return h + '</section>';
    }
    function reqFilesHtml(n) {
      const files = (outE.get(n.id) || []).filter((e) => e.kind === 'reqs').map((e) => nodesById.get(e.to)).filter(isReqFile);
      const row = (type, label, hint) => {
        const fs = files.filter((f) => f.nodetype === type); const st = LEVELS[type];
        return `<div class="g-rfile ft-${type}" data-rtype="${type}"><span class="g-chipsw ${type}" style="--c:${st.color};--b:${st.border}"></span>
          <div class="g-rfile-main"><b>${esc(label)}</b>${fs.length ? fs.map((f) => `<div class="g-rfile-f">${nodeLink(f.id)}<span class="g-count" title="requirements in this file">${f.req_count || 0}</span></div>`).join('') : `<div class="muted">${esc(hint)}</div>`}</div>
          <span class="g-rfile-tools" data-rtools="${type}" data-file="${esc(fs[0] ? fs[0].id : '')}"></span></div>`;
      };
      return `<h4>Requirements</h4><div class="g-rfiles">${row('bizreq', 'Business', 'no file yet — Add creates it')}${row('techreq', 'Technical', 'no file yet — Add creates it')}</div>`;
    }
    function reqDetail(n) {
      const f = nodesById.get(n.file); const rs = REQ_STYLE[n.file_type] || REQ_STYLE.bizreq;
      const sibs = f && f.reqs ? f.reqs : []; const at = sibs.findIndex((r) => r.id === n.id);
      let h = `<h2>${n.key ? `<span class="g-rkey ft-${esc(n.file_type || 'bizreq')}">${esc(n.key)}</span>` : ''}<span>${esc(n.title || n.name)}</span></h2>
        <div class="g-badges"><span class="badge" style="background:${rs.color}">${n.file_type === 'techreq' ? 'Technical requirement' : 'Business requirement'}</span>${statusPill(n)}${n.file_state !== 'synced' ? fsBadge(n) : ''}
        <button type="button" class="hidenode" data-hide="${esc(n.id)}" title="Take this node off the drawing until Show hidden (this browser tab only)">Hide</button></div>`;
      h += '<div class="g-actions-slot"></div>';
      h += n.text ? `<div class="g-rtext g-rtext-big">${md(n.text)}</div>` : '<div class="simple missing">No text yet.</div>';
      h += `<h4>In file</h4><div class="g-rinfile">${f ? nodeLink(f.id) : `<span class="muted">${esc(n.file || '?')}</span>`}<span class="muted">${n.file_type === 'techreq' ? 'technical' : 'business'} requirements file${at >= 0 ? ` · section ${at + 1} of ${sibs.length}` : ''}</span></div>`;
      const owners = f ? (inE.get(f.id) || []).filter((e) => e.kind === 'reqs').map((e) => e.from) : [];
      if (owners.length) h += `<h4>Required by</h4>${owners.map((x) => `<div>${nodeLink(x)}</div>`).join('')}`;
      if (n.derived) h += '<p class="muted">This section has no id yet: the server gives it one the next time the file is written.</p>';
      h += `<dl class="g-facts"><dt>key</dt><dd>${esc(n.key || '—')}</dd><dt>file</dt><dd class="g-mono">${esc(f ? f.path : String(n.path || '').replace(/#.*$/, ''))}</dd><dt>id</dt><dd class="g-id" title="click to copy">${esc(n.id)}</dd></dl>`;
      return h;
    }
    function wireReqTable(box) {
      const toggle = (id, open) => {
        if (open == null) open = !EXPANDED.has(id);
        if (open) EXPANDED.add(id); else EXPANDED.delete(id);
        const tr = box.querySelector(`tr.g-rrow[data-req="${CSS.escape(id)}"]`); const more = box.querySelector(`tr[data-req-more="${CSS.escape(id)}"]`);
        if (tr) { tr.classList.toggle('open', open); tr.setAttribute('aria-expanded', String(open)); }
        if (more) more.classList.toggle('hidden', !open);
      };
      box.querySelectorAll('tr.g-rrow').forEach((tr) => {
        tr.onclick = (ev) => { if (ev.target.closest('a, button, .g-rst.edit')) return; toggle(tr.dataset.req); };
        tr.onkeydown = (ev) => {
          if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); ev.stopPropagation(); toggle(tr.dataset.req); }
          else if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') { ev.preventDefault(); ev.stopPropagation(); const rows = [...box.querySelectorAll('tr.g-rrow')]; const i = rows.indexOf(tr); const nx = rows[i + (ev.key === 'ArrowDown' ? 1 : -1)]; if (nx) nx.focus(); }
        };
      });
      const all = box.querySelector('.g-rall');
      if (all) {
        const go = () => { const ids = [...box.querySelectorAll('tr.g-rrow')].map((t) => t.dataset.req); const open = !ids.every((id) => EXPANDED.has(id)); ids.forEach((id) => toggle(id, open)); all.textContent = open ? 'fold all' : 'open all'; };
        all.onclick = go; all.onkeydown = (ev) => { if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); ev.stopPropagation(); go(); } };
      }
    }

    function flowCard(e) {
      const text = stepText(e);
      return `<div class="card"><b>Step ${esc(stepName(e))}</b> <span class="muted">${esc(e.kind)} flow${e.branch ? ', branch ' + esc(e.branch) : ''}</span>${statusTag(e.status)}${e.stored ? ' <span class="muted">(stored here)</span>' : ''}
        <div>${nodeLink(e.source)} → ${nodeLink(e.target)}</div>
        ${e.plain ? `<div class="plainline">${esc(e.plain)}</div><div class="what detail">${esc(text)}</div>` : `<div class="what">${esc(text)}</div>`}
        ${e.via ? `<div class="muted">Via: <code>${esc(e.via)}</code></div>` : ''}${evidenceHtml(e.evidence)}</div>`;
    }
    function usecaseSteps(id) {
      const u = ucById.get(id); if (!u) return '';
      let h = '';
      if (u.actors.length) h += `<h4>Driven by</h4>${u.actors.map((a) => `<div>${nodeLink(a)}</div>`).join('')}`;
      if (state.uc !== id) h += `<p><a class="uc" data-uc="${esc(id)}">Show this journey on its own →</a></p>`;
      if (!u.has_flowmap) return h + `<p class="muted">No flowmap yet: the usecase agent writes one (summary, sequence, numbered process and data steps).</p>`;
      for (const [kind, title] of [['process', 'Process flow'], ['data', 'Data flow']]) {
        const list = (flowsByUc.get(id) || []).filter((e) => e.kind === kind);
        if (!list.length) continue;
        h += `<h4>${title} (${list.length} steps)</h4>`; list.forEach((e) => { h += flowCard(e); });
      }
      return h;
    }
    function edgeDetail(m) {
      if (!m) return '';
      if (m.type === 'flow') { const u = ucById.get(m.usecase); return `<h2>Step ${esc(stepName(m))} — ${esc(m.kind)} flow</h2><p class="muted">Use case: <a class="uc" data-uc="${esc(m.usecase)}">${esc(u ? u.name : m.usecase)}</a></p>${flowCard(m)}`; }
      if (m.type === 'start') { const u = ucById.get(m.source); return `<h2>Where the use case starts</h2><p><a class="uc" data-uc="${esc(m.source)}">${esc(u ? u.name : m.source)}</a> → ${nodeLink(m.target)}</p>`; }
      const k = EDGE_KINDS[m.kind] || { label: m.kind, color: '#888' };
      const from = nodesById.get(m.source); const to = nodesById.get(m.target);
      const owner = (m.kind === 'supplies') ? to : from;
      const where = { codenodes: 'children.codenodes', tests: 'children.tests', reqs: 'children.reqs', supplies: 'connects.from', connects: 'connects.to', drives: 'drives (or the use case\'s actors)', touches: 'touches', uses: 'children.codenodes (of the use case)', contains: 'its body, as a ## section' }[m.kind] || '';
      if (m.kind === 'contains') {
        return `<h2><span class="g-eswatch big" style="background:${k.color}"></span>${esc(k.label)}</h2>
          <div class="g-edgeends"><div>${nodeLink(m.source)}<span class="muted">${esc((LEVELS[from && from.level] || {}).label || '')} file</span></div><div class="g-edgearrow">holds →</div><div>${nodeLink(m.target)}<span class="muted">requirement</span></div></div>
          <div class="g-actions-slot"></div>
          <p class="muted">The requirement is a <code>##</code> section of <b>${esc(from ? from.name : '')}</b>'s file. Drag the file end of the selected line onto another bizreq or techreq file to move the requirement there (it keeps its id).</p>`;
      }
      return `<h2><span class="g-eswatch big" style="background:${k.color}"></span>${esc(k.label)}</h2>
        <div class="g-edgeends"><div>${nodeLink(m.source)}<span class="muted">${esc((LEVELS[from && from.level] || {}).label || '')}</span></div><div class="g-edgearrow">${esc(VERB_OUT[m.kind] || m.kind)} →</div><div>${nodeLink(m.target)}<span class="muted">${esc((LEVELS[to && to.level] || {}).label || '')}</span></div></div>
        <div class="g-actions-slot"></div>
        <p class="muted">Recorded in <b>${esc(owner ? owner.name : '')}</b>'s file, under <code>${esc(where)}</code>. Drag either end of the selected edge onto another node to move it.</p>`;
    }

    // ------------------------------------------------------------------ search
    const NUM_TOKEN = /^d?\d+\*?$/;
    let SEARCH_INDEX = [];
    function buildSearchIndex() {
      SEARCH_INDEX = [];
      for (const n of G.nodes) {
        SEARCH_INDEX.push({ type: 'node', n, fields: [['name', n.name], ['description', n.desc], ['path', n.path], ['type', (LEVELS[n.level] || {}).label], ['id', n.id]].concat(n.nodetype === 'req' ? [['text', n.text]] : [])
          .filter((f) => f[1]).map((f) => [f[0], String(f[1]), String(f[1]).toLowerCase()]) });
      }
      flowsByUc.forEach((list) => list.forEach((e) => {
        const f = [['plain', e.plain], ['what', e.what], ['data', e.data], ['via', e.via], ['from', nameOf(e.source)], ['to', nameOf(e.target)]];
        SEARCH_INDEX.push({ type: 'flow', e, fields: f.filter((x) => x[1]).map((x) => [x[0], String(x[1]), String(x[1]).toLowerCase()]), stepWords: ['step', stepName(e).toLowerCase(), String(e.step)] });
      }));
    }
    const ENDPOINT_FIELDS = new Set(['from', 'to']);
    const searchTokens = (q) => q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    function matchesFor(q) {
      const toks = searchTokens(q); const empty = { nodes: [], edges: [], toks };
      if (!toks.length || (q.trim().length < 2 && !NUM_TOKEN.test(toks[0]))) return empty;
      const nodes = []; const edges = [];
      for (const ent of SEARCH_INDEX) {
        let primary = false; let stepExact = false; let ok = true;
        for (const t of toks) {
          if (ent.stepWords && NUM_TOKEN.test(t)) { if (ent.stepWords.includes(t.replace('*', ''))) { stepExact = true; primary = true; continue; } ok = false; break; }
          if (ent.stepWords && ent.stepWords[0].includes(t)) continue;
          const f = ent.fields.find((x) => x[2].includes(t));
          if (!f) { ok = false; break; }
          if (!ENDPOINT_FIELDS.has(f[0])) primary = true;
        }
        if (!ok) continue;
        if (ent.type === 'node') {
          const inName = toks.every((t) => ent.n.name.toLowerCase().includes(t));
          nodes.push({ ent, hidden: cy.getElementById(ent.n.id).empty(), score: [inName ? 0 : 1, LEVEL_ORDER.indexOf(ent.n.level)] });
        } else {
          const e = ent.e; const here = e.usecase === state.uc;
          edges.push({ ent, hidden: !here, score: [stepExact ? 0 : 1, here ? 0 : 1, primary ? 0 : 1, e.kind === 'data' ? 1 : 0, typeof e.step === 'number' ? e.step : 1e9] });
        }
      }
      const cmp = (a, b) => { for (let i = 0; i < a.score.length; i++) if (a.score[i] !== b.score[i]) return a.score[i] - b.score[i]; return 0; };
      nodes.sort(cmp); edges.sort(cmp);
      return { nodes, edges, toks };
    }
    function hi(text, toks) {
      const s = String(text == null ? '' : text); const low = s.toLowerCase(); const r = [];
      toks.forEach((t) => { const w = t.replace('*', ''); if (!w) return; let i = low.indexOf(w); while (i >= 0) { r.push([i, i + w.length]); i = low.indexOf(w, i + w.length); } });
      if (!r.length) return esc(s);
      r.sort((a, b) => a[0] - b[0]); let out = ''; let at = 0;
      for (const [a, b] of r) { if (b <= at) continue; const from = Math.max(a, at); out += esc(s.slice(at, from)) + '<mark>' + esc(s.slice(from, b)) + '</mark>'; at = b; }
      return out + esc(s.slice(at));
    }
    const SEARCH_CAP = { nodes: 10, edges: 14 };
    let SEARCH_ROWS = []; let SEARCH_ACTIVE = 0;
    function applySearch(showList) {
      const q = $('search').value; const res = matchesFor(q); const toks = res.toks;
      cy.elements().removeClass('match');
      res.nodes.forEach((r) => cy.getElementById(r.ent.n.id).addClass('match'));
      if (res.edges.length) { const hit = new Set(res.edges.map((r) => r.ent.e.id)); EDGES.forEach((m, id) => { if (m.type === 'flow' && hit.has(m.id)) cy.getElementById(id).addClass('match'); }); }
      const list = $('searchlist');
      SEARCH_ROWS = [...res.nodes.slice(0, SEARCH_CAP.nodes), ...res.edges.slice(0, SEARCH_CAP.edges)]; SEARCH_ACTIVE = 0;
      if (!showList || !q.trim()) { list.classList.add('hidden'); return res; }
      let h = ''; let i = 0;
      const more = (n) => (n > 0 ? `<div class="more">${n} more… type another word to narrow</div>` : '');
      if (res.nodes.length) {
        h += `<div class="grp">Nodes (${res.nodes.length})</div>`;
        res.nodes.slice(0, SEARCH_CAP.nodes).forEach((r) => {
          const n = r.ent.n;
          h += `<div class="row" data-i="${i++}">${dot(n.id)}<div class="txt"><div><b>${hi(n.name, toks)}</b> <span class="muted">${esc((LEVELS[n.level] || {}).label || n.level)}</span></div>`
            + (n.desc ? `<div class="sm">${hi(trunc(n.desc, 110), toks)}</div>` : '') + (r.hidden ? '<div class="hid">not in this view — click to show</div>' : '') + '</div></div>';
        });
        h += more(res.nodes.length - SEARCH_CAP.nodes);
      }
      if (res.edges.length) {
        h += `<div class="grp">Use-case steps (${res.edges.length})</div>`;
        res.edges.slice(0, SEARCH_CAP.edges).forEach((r) => {
          const e = r.ent.e; const u = ucById.get(e.usecase);
          h += `<div class="row" data-i="${i++}"><span class="bar" style="background:${e.kind === 'process' ? '#3b82f6' : '#f97316'}"></span><div class="txt"><div><b>${hi(`Step ${stepName(e)} · ${nameOf(e.source)} → ${nameOf(e.target)}`, toks)}</b></div>`
            + `<div class="sm">${hi(trunc(e.plain || stepText(e), 110), toks)}</div>` + (u && e.usecase !== state.uc ? `<div class="muted">${esc(u.name)}</div>` : '') + '</div></div>';
        });
        h += more(res.edges.length - SEARCH_CAP.edges);
      }
      list.innerHTML = h || '<div class="more">No match</div>';
      list.classList.remove('hidden');
      list.querySelectorAll('.row').forEach((d) => {
        d.onmousedown = (ev) => { ev.preventDefault(); pickResult(+d.dataset.i); };
        d.onmousemove = () => { if (SEARCH_ACTIVE !== +d.dataset.i) { SEARCH_ACTIVE = +d.dataset.i; markActive(false); } };
      });
      markActive(false);
      return res;
    }
    function markActive(scroll) {
      const rows = $('searchlist').querySelectorAll('.row');
      rows.forEach((d) => d.classList.toggle('active', +d.dataset.i === SEARCH_ACTIVE));
      if (scroll && rows[SEARCH_ACTIVE]) rows[SEARCH_ACTIVE].scrollIntoView({ block: 'nearest' });
    }
    function pickResult(i) {
      const r = SEARCH_ROWS[i]; if (!r) return;
      $('searchlist').classList.add('hidden');
      if (r.ent.type === 'node') focusNode(r.ent.n.id); else focusFlow(r.ent.e);
    }
    function focusFlow(e) {
      if (state.uc !== e.usecase) setUc(e.usecase);
      if (state.flow !== 'both' && state.flow !== e.kind) state.flow = 'both';
      if (state.branch && e.branch && e.branch !== state.branch) state.branch = '';
      render();
      let eid = null; EDGES.forEach((m, id) => { if (!eid && m.type === 'flow' && m.id === e.id) eid = id; });
      if (!eid) return;
      select({ kind: 'edge', id: eid }, { noPan: true });
      cy.animate({ center: { eles: cy.getElementById(eid) }, zoom: Math.min(cy.maxZoom(), Math.max(cy.zoom(), 1)) }, { duration: 300 });
    }
    function focusEdgeId(eid) {
      let el = cy.getElementById(eid);
      if (el.empty() && eid.startsWith('e:')) {
        const id = eid.slice(2); const e = G.edges.find((x) => x.id === id);
        if (e) { [e.from, e.to].forEach((x) => { const n = nodesById.get(x); if (n) { HIDE_TYPES.delete(n.level); HIDDEN.delete(x); } }); if (state.uc) setUc(''); render({ keep: snapshot() }); el = cy.getElementById(eid); }
      }
      if (el.empty()) return false;
      select({ kind: 'edge', id: eid }, { noPan: true });
      cy.animate({ center: { eles: el } }, { duration: 250 });
      return true;
    }
    $('search').addEventListener('input', () => applySearch(true));
    $('search').addEventListener('focus', () => applySearch(true));
    $('search').addEventListener('blur', () => $('searchlist').classList.add('hidden'));
    $('search').addEventListener('keydown', (ev) => {
      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
        ev.preventDefault(); if ($('searchlist').classList.contains('hidden')) applySearch(true);
        if (!SEARCH_ROWS.length) return;
        SEARCH_ACTIVE = (SEARCH_ACTIVE + (ev.key === 'ArrowDown' ? 1 : -1) + SEARCH_ROWS.length) % SEARCH_ROWS.length; markActive(true);
      } else if (ev.key === 'Enter') { ev.preventDefault(); if (SEARCH_ROWS.length) pickResult(SEARCH_ACTIVE); } else if (ev.key === 'Escape') { $('search').value = ''; applySearch(false); $('search').blur(); }
    });
    const visible = () => !!ROOT.offsetParent;
    onDoc('keydown', (ev) => {
      if (!visible() || (window.IterKit && IterKit.dialogOpen())) return;
      const typing = window.IterKit ? IterKit.typing() : false;
      const find = (ev.metaKey || ev.ctrlKey) && !ev.altKey && !ev.shiftKey && (ev.key === 'f' || ev.key === 'F');
      if (find || (ev.key === '/' && !typing && !ev.metaKey && !ev.ctrlKey)) { ev.preventDefault(); $('search').focus(); $('search').select(); applySearch(true); return; }
      if (typing || ev.metaKey || ev.ctrlKey || ev.altKey) return;
      if (ev.key === 'Escape' && SELECTED && !(window.IterGraphEdit && IterGraphEdit.drawing)) { select(null); return; }
      if (ev.key === 'f' || ev.key === 'F') { ev.preventDefault(); fit(); }
      else if (ev.key === 'r' || ev.key === 'R') { ev.preventDefault(); render(); }
      else if (ev.key === '?') { ev.preventDefault(); openHelp(); }
    });

    // ------------------------------------------------------------------ controls
    function fillPicker() {
      const sel = $('ucpick');
      sel.innerHTML = '<option value="">Whole project</option>'
        + (USECASES.length ? `<optgroup label="Use cases">${USECASES.map((u) => `<option value="${esc(u.id)}">${esc(u.name)}${u.has_flowmap ? '' : ' (no flowmap yet)'}</option>`).join('')}</optgroup>` : '')
        + (CTX.openSettings ? '<optgroup label="System"><option value="__settings">⚙ Settings graph</option></optgroup>' : '');
    }
    function renderChips() {
      const counts = {}; G.nodes.forEach((n) => { counts[n.level] = (counts[n.level] || 0) + 1; });
      $('chips').innerHTML = LEVEL_ORDER.map((k) => {
        const st = LEVELS[k]; const on = !HIDE_TYPES.has(k); const c = counts[k] || 0;
        return `<button type="button" class="g-chip ${on ? 'on' : ''} ${c ? '' : 'zero'}" data-type="${k}" aria-pressed="${on}" title="${esc(st.long)} — ${c} on this project. Click to ${on ? 'hide' : 'show'}; alt-click to show only this type">
          <span class="g-chipsw ${esc(k)}" style="--c:${st.color};--b:${st.border}"></span>${esc(st.label)}<em>${c}</em></button>`;
      }).join('');
      const presetOn = (p) => LEVEL_ORDER.filter((k) => !HIDE_TYPES.has(k) && !(p.optional || []).includes(k)).join(',') === LEVEL_ORDER.filter((k) => p.show.includes(k)).join(',');
      $('presets').innerHTML = PRESETS.map((p) => `<button type="button" class="g-preset ${presetOn(p) ? 'on' : ''}" data-preset="${p.key}" title="${esc(p.tip)}">${esc(p.label)}</button>`).join('');
      // the "requirement sections" toggle: offered while a requirements file type is on screen and there are sections to draw
      const fileShown = !HIDE_TYPES.has('bizreq') || !HIDE_TYPES.has('techreq');
      ROOT.querySelector('.reqsecopt').classList.toggle('hidden', !fileShown || !counts.req);
      $('reqsec').checked = !HIDE_TYPES.has('req');
    }
    function syncControls() {
      fillPicker();
      $('ucpick').value = state.uc;
      const uc = state.uc ? ucById.get(state.uc) : null;
      ROOT.querySelectorAll('.ucopt').forEach((el) => el.classList.toggle('hidden', !uc));
      ROOT.querySelectorAll('.wholeopt').forEach((el) => el.classList.toggle('hidden', !!uc || state.layout !== 'cluster'));
      $('dim').checked = state.dim; $('own').checked = state.own;
      ROOT.querySelectorAll('#g-layouts button').forEach((b) => {
        b.classList.toggle('on', b.dataset.layout === state.layout);
        if (b.dataset.layout === 'sequence') { b.disabled = !uc || state.dim; b.classList.toggle('hidden', !uc); }
      });
      ROOT.querySelectorAll('#g-flowkind button').forEach((b) => b.classList.toggle('on', b.dataset.flow === state.flow));
      const br = $('branch'); const branches = uc ? uc.branches : [];
      br.classList.toggle('hidden', !branches.length);
      br.innerHTML = '<option value="">All branches</option>' + branches.map((b) => `<option value="${esc(b)}">Branch: ${esc(b)}</option>`).join('');
      br.value = branches.includes(state.branch) ? state.branch : '';
      renderChips(); renderSummary();
    }
    function setUc(uc) {
      state.uc = uc; state.branch = '';
      if (!state.uc && state.layout === 'sequence') state.layout = 'cluster';
      if (state.uc && (ucById.get(state.uc) || {}).has_flowmap && state.layout === 'cluster') state.layout = 'sequence';
      SELECTED = null; $('detail').classList.add('hidden'); cy.resize(); emitSel();
    }
    $('ucpick').addEventListener('change', (ev) => {
      if (ev.target.value === '__settings') { ev.target.value = state.uc; if (CTX.openSettings) CTX.openSettings(); return; }
      setUc(ev.target.value); render();
    });
    $('dim').addEventListener('change', (ev) => { state.dim = ev.target.checked; if (state.dim && state.layout === 'sequence') state.layout = 'tiered'; render(); });
    $('own').addEventListener('change', (ev) => { state.own = ev.target.checked; render({ keep: snapshot() }); });
    ROOT.querySelector('#g-layouts').addEventListener('click', (ev) => { const b = ev.target.closest('button[data-layout]'); if (b && !b.disabled) { state.layout = b.dataset.layout; render(); } });
    ROOT.querySelector('#g-flowkind').addEventListener('click', (ev) => { const b = ev.target.closest('button[data-flow]'); if (b) { state.flow = b.dataset.flow; render(); } });
    $('branch').addEventListener('change', (ev) => { state.branch = ev.target.value; render(); });
    $('chips').addEventListener('click', (ev) => {
      const b = ev.target.closest('button[data-type]'); if (!b) return;
      const k = b.dataset.type;
      if (ev.altKey) { HIDE_TYPES = new Set(LEVEL_ORDER.filter((x) => x !== k)); } else if (HIDE_TYPES.has(k)) HIDE_TYPES.delete(k); else HIDE_TYPES.add(k);
      render({ keep: snapshot() });
    });
    $('reqsec').addEventListener('change', (ev) => { if (ev.target.checked) HIDE_TYPES.delete('req'); else HIDE_TYPES.add('req'); render({ keep: snapshot() }); });
    $('presets').addEventListener('click', (ev) => {
      const b = ev.target.closest('button[data-preset]'); if (!b) return;
      const p = PRESETS.find((x) => x.key === b.dataset.preset); if (!p) return;
      const keepReq = (p.optional || []).includes('req') && !HIDE_TYPES.has('req'); // a preset leaves the sections toggle as it was
      HIDE_TYPES = new Set(LEVEL_ORDER.filter((x) => !p.show.includes(x) && !(keepReq && x === 'req')));
      if (p.layout && !state.uc) state.layout = p.layout;
      render();
    });
    const wheelPx = (ev, d) => d * (ev.deltaMode === 1 ? 16 : ev.deltaMode === 2 ? $('cy').clientHeight : 1);
    let gestureStart = 1; let inGesture = false;
    $('cy').addEventListener('wheel', (ev) => {
      ev.preventDefault();
      const box = $('cy').getBoundingClientRect(); const at = { x: ev.clientX - box.left, y: ev.clientY - box.top };
      if (ev.ctrlKey || ev.metaKey) {
        if (inGesture) return;
        const d = Math.max(-25, Math.min(25, wheelPx(ev, ev.deltaY)));
        cy.zoom({ level: Math.min(cy.maxZoom(), Math.max(cy.minZoom(), cy.zoom() * Math.exp(-d * 0.01))), renderedPosition: at });
      } else {
        let dx = wheelPx(ev, ev.deltaX); let dy = wheelPx(ev, ev.deltaY);
        if (ev.shiftKey && !dx) { dx = dy; dy = 0; }
        cy.panBy({ x: -dx, y: -dy });
      }
    }, { passive: false });
    $('cy').addEventListener('gesturestart', (ev) => { ev.preventDefault(); inGesture = true; gestureStart = cy.zoom(); });
    $('cy').addEventListener('gesturechange', (ev) => { ev.preventDefault(); const box = $('cy').getBoundingClientRect(); cy.zoom({ level: Math.min(cy.maxZoom(), Math.max(cy.minZoom(), gestureStart * ev.scale)), renderedPosition: { x: ev.clientX - box.left, y: ev.clientY - box.top } }); });
    $('cy').addEventListener('gestureend', (ev) => { ev.preventDefault(); inGesture = false; });
    const fit = () => cy.animate({ fit: { eles: cy.elements().not('.kit-ep'), padding: 30 } }, { duration: 250 });
    $('fit').addEventListener('click', fit);
    $('relayout').addEventListener('click', () => render());
    $('showhidden').addEventListener('click', () => showAllHidden());
    $('help').addEventListener('click', () => openHelp());
    $('detail').addEventListener('click', (ev) => {
      const idEl = ev.target.closest('.g-id');
      if (idEl && navigator.clipboard) { navigator.clipboard.writeText(idEl.textContent).then(() => window.IterKit && IterKit.toast('Copied the id', { kind: 'ok', ms: 1400 })).catch(() => {}); }
    });
    function openHelp() {
      if (!window.IterKit) return;
      const groups = [
        ['Moving around', [['/ / Mod+F', 'Search nodes and use-case steps'], ['F', 'Fit the drawing on screen'], ['R', 'Run the layout again'], ['Esc', 'Clear the selection, close a menu'], ['Scroll / two fingers', 'Pan'], ['Mod+scroll / pinch', 'Zoom'], ['?', 'This help']]],
        ['Show and hide', [['Click a type chip', 'Show or hide that node type'], ['Alt+click a chip', 'Show only that type'], ['Network map', 'Code and connections only, left to right']]],
      ].concat(window.IterGraphEdit && IterGraphEdit.helpGroups ? IterGraphEdit.helpGroups() : []);
      IterKit.help('Project graph — shortcuts', groups, 'The type chips and the view live in the page address, so a copied link opens the same picture.');
    }
    window.addEventListener('hashchange', () => {
      const hp = new URLSearchParams(location.hash.slice(1)).get('p');
      if (hp && CTX.project && hp !== CTX.project) return;
      if (!visible()) return;
      readHash(); render();
    }, { signal: SIGNAL });

    // ------------------------------------------------------------------ legend + summary line
    function renderLegend() {
      const lineSvg = (x) => `<svg width="30" height="8" aria-hidden="true"><line x1="1" y1="4" x2="23" y2="4" stroke="${x.color}" stroke-width="${x.width > 2 ? 3 : 2}" ${x.line === 'dashed' ? 'stroke-dasharray="5 3"' : x.line === 'dotted' ? 'stroke-dasharray="1.5 2.5"' : ''}/><polygon points="23,0 30,4 23,8" fill="${x.color}"/></svg>`;
      const nodes = LEVEL_ORDER.map((k) => `<div class="item"><span class="g-chipsw ${k}" style="--c:${LEVELS[k].color};--b:${LEVELS[k].border}"></span>${esc(LEVELS[k].long)}</div>`).join('');
      const edges = Object.values(EDGE_KINDS).map((x) => `<div class="item">${lineSvg(x)}${esc(x.label)}</div>`).join('')
        + `<div class="item">${lineSvg({ color: '#3b82f6', width: 3 })}use-case process step</div><div class="item">${lineSvg({ color: '#f97316', width: 3 })}use-case data step</div>`;
      const fs = '<div class="item"><span class="g-fsw dashed" style="border-color:#e0b341"></span>⟳ pending sync (an engine writes the file)</div><div class="item"><span class="g-fsw dashed" style="border-color:#4da3ff"></span>✎ designed (no repository yet)</div><div class="item"><span class="g-fsw dashed" style="border-color:#e05252;opacity:.5"></span>✕ pending delete</div><div class="item"><span class="g-fsw" style="border-color:#4ade80"></span>test passing</div><div class="item"><span class="g-fsw" style="border-color:#f87171;background:#7f1d1d"></span>test failing</div>';
      $('legend').innerHTML = `<button type="button" id="g-legendToggle" class="legendtoggle" aria-expanded="false">Legend</button>`
        + `<div class="legendbody"><div class="row"><span class="lbl">Nodes</span>${nodes}</div><div class="row"><span class="lbl">Edges</span>${edges}</div><div class="row"><span class="lbl">State</span>${fs}</div>`
        + '<div class="row note"><span class="lbl">Note</span><span>A connection is a <b>type</b> of link (an API call over HTTP, an event, a stream): the parts that <b>supply</b> it point into its diamond, and it <b>connects</b> to the parts it reaches. Everything here is a file: an edit shows at once and an engine writes the file within seconds.</span></div></div>';
      let open = false;
      try { open = localStorage.getItem('iter5.graph.legendOpen') === '1'; } catch (e) { /* storage blocked */ }
      const apply = () => {
        $('legend').classList.toggle('collapsed', !open);
        $('legendToggle').setAttribute('aria-expanded', String(open));
        $('legendToggle').textContent = open ? 'Legend ▾' : 'Legend ▸';
        cy.resize();
      };
      $('legendToggle').addEventListener('click', () => { open = !open; try { localStorage.setItem('iter5.graph.legendOpen', open ? '1' : '0'); } catch (e) { /* storage blocked */ } apply(); });
      apply();
    }
    function renderSummary() {
      const by = {}; G.nodes.forEach((n) => { by[n.level] = (by[n.level] || 0) + 1; });
      const parts = LEVEL_ORDER.filter((k) => by[k] && k !== 'req').map((k) => `${by[k]} ${LEVELS[k].label.toLowerCase()}${by[k] === 1 ? '' : (k === 'philosophy' ? '' : 's')}`.replace(/philosophy$/, by[k] === 1 ? 'philosophy' : 'philosophies'));
      if (by.req) parts.push(`${by.req} requirement${by.req === 1 ? '' : 's'}`);
      // nodes = files; requirement sections and their "contains" lines are counted on their own
      const files = G.nodes.length - (by.req || 0); const links = G.edges.filter((e) => e.kind !== 'contains').length;
      $('summary').textContent = `${files} nodes · ${links} links${parts.length ? ' — ' + parts.join(' · ') : ''}`;
      $('summary').title = $('summary').textContent;
    }

    // ------------------------------------------------------------------ go
    index(G0);
    readHash();
    renderLegend();
    render();

    /** Fresh data from the server: same picture, new facts. */
    function update(G2) {
      const snap = snapshot();
      index(G2);
      if (state.uc && !ucById.has(state.uc)) state.uc = '';
      if (SELECTED && SELECTED.kind === 'node' && !nodesById.has(SELECTED.id)) { SELECTED = null; $('detail').classList.add('hidden'); emitSel(); }
      render({ keep: snap });
      if (SELECTED && !$('detail').classList.contains('hidden')) {
        const sc = $('detail').scrollTop; renderDetail(); $('detail').scrollTop = sc;
      }
    }
    const api = {
      cy, root: ROOT, update, render: (o) => render(o), snapshot,
      get model() { return { G, nodesById, outE, inE, usecases: USECASES }; },
      get selected() { return SELECTED ? Object.assign({}, SELECTED) : null; },
      edgeRecord: (eid) => EDGES.get(eid) || null,
      select, focusNode, focusEdgeId, hide: hideNode, banner: (h, ms) => {
        const b = $('banner'); b.innerHTML = h; b.classList.toggle('hidden', !h);
        if (ms) setTimeout(() => { if (b.innerHTML === h) { b.textContent = LASTING; b.classList.toggle('hidden', !LASTING); } }, ms);
      },
      showType: (lv) => { if (HIDE_TYPES.has(lv)) { HIDE_TYPES.delete(lv); render({ keep: snapshot() }); } },
      typeShown: (lv) => !HIDE_TYPES.has(lv),
      /** Open a requirement's row in the table on screen (and keep it open across redraws). */
      expandReq: (id) => {
        EXPANDED.add(id);
        const tr = $('detail').querySelector(`tr.g-rrow[data-req="${CSS.escape(id)}"]`); const more = $('detail').querySelector(`tr[data-req-more="${CSS.escape(id)}"]`);
        if (tr) { tr.classList.add('open'); tr.setAttribute('aria-expanded', 'true'); tr.scrollIntoView({ block: 'nearest' }); }
        if (more) more.classList.remove('hidden');
      },
      openHelp,
    };
    window.__usecaseMap = { cy, state, render }; // scripted checks
    window.IterGraph.edgeInfo = (id) => EDGES.get(id) || null;
    window.IterGraph.hide = (id) => hideNode(id);
    window.IterGraph.settle = () => { cy.resize(); if (SEQ_PINNED) { showBanner(LASTING, seqStart()); return; } cy.fit(cy.elements().not('.kit-ep'), VIEW && VIEW.uc && !state.dim ? 55 : 30); if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); } };
    window.IterGraph.focus = (q) => {
      const k = String(q || '').toLowerCase(); if (!k) return false;
      const all = [...nodesById.values()];
      const hit = all.find((n) => n.id.toLowerCase() === k) || all.find((n) => normPath(n.path).toLowerCase() === normPath(k))
        || all.find((n) => String(n.name || '').toLowerCase() === k) || all.find((n) => String(n.name || '').toLowerCase().includes(k)) || all.find((n) => String(n.path || '').toLowerCase().includes(k));
      if (!hit) return false;
      return focusNode(hit.id, { flash: true });
    };
    return api;
  }
}());
