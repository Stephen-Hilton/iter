/*
 * iter5 Settings graph (spec §7, §9): every setting lives on a node or an
 * edge. Nodes: iter_data, iter_engine, project, workitem_type, agent,
 * agent_tools, user, account, provider — each type also has a non-deletable
 * placeholder "<type>:_deactivated". Edges carry the settings that join two
 * things (an engine serves a project with a topdir, an account bills a project
 * with switch/stop %…). Moving an edge end onto a placeholder keeps the edge
 * and its settings but makes it inactive.
 *
 *   GET /api/settings/graph · POST /api/settings/nodes · PATCH/DELETE /api/settings/nodes/{id}
 *   POST /api/settings/edges · PATCH/DELETE /api/settings/edges/{id} · POST /api/settings/edges/{id}/copy
 *
 * Editing is admin-only; everyone else reads. Mounted by index.html as the
 * Settings tab: IterSettings.mount(el, ctx), .show(ctx), .focus(nodeId).
 */
(function () {
  'use strict';
  const K = () => window.IterKit;
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const enc = encodeURIComponent;
  const TYPES = {
    iter_data: { label: 'iter_data', long: 'iter_data — this server', color: '#475569', border: '#94a3b8', shape: 'octagon', w: 46, h: 46, col: 0 },
    user: { label: 'User', long: 'User — sees projects, owns engines', color: '#b45309', border: '#fcd34d', shape: 'ellipse', w: 30, h: 30, col: 0 },
    iter_engine: { label: 'Engine', long: 'iter_engine — one per machine', color: '#2563eb', border: '#93c5fd', shape: 'round-rectangle', w: 42, h: 28, col: 1 },
    agent_tools: { label: 'Agent tools', long: 'Agent tools — instructions an agent looks up', color: '#0f766e', border: '#5eead4', shape: 'round-rectangle', w: 24, h: 16, col: 1 },
    account: { label: 'Account', long: 'Account — an LLM subscription', color: '#be185d', border: '#f9a8d4', shape: 'round-diamond', w: 32, h: 32, col: 2 },
    agent: { label: 'Agent', long: 'Agent — a named worker (code, plan, test…)', color: '#0d9488', border: '#99f6e4', shape: 'ellipse', w: 32, h: 32, col: 2 },
    project: { label: 'Project', long: 'Project', color: '#7c3aed', border: '#c4b5fd', shape: 'round-rectangle', w: 50, h: 32, col: 3 },
    provider: { label: 'Provider', long: 'Provider — claude, mock…', color: '#0891b2', border: '#67e8f9', shape: 'hexagon', w: 30, h: 28, col: 3 },
    workitem_type: { label: 'Work item state', long: 'Work item type — one per state', color: '#475569', border: '#cbd5e1', shape: 'round-tag', w: 30, h: 20, col: 4 },
  };
  const TYPE_ORDER = ['iter_data', 'user', 'iter_engine', 'account', 'provider', 'project', 'agent', 'agent_tools', 'workitem_type'];
  const COL_ORDER = ['iter_data', 'user', 'iter_engine', 'agent_tools', 'account', 'agent', 'project', 'provider', 'workitem_type'];
  /** spec §7 / iter_core::settings::EDGE_TYPES — (from, to) pairs are unique, so the type is inferable. */
  const EDGE_TYPES = [
    { edge: 'serves', from: 'iter_engine', to: 'project', desc: 'the engine runs the project', keys: { topdir: '', read_only: false } },
    { edge: 'bills', from: 'account', to: 'project', desc: 'the project may use the account', keys: { order: 1, switch: 80, stop: 95, model: '' } },
    { edge: 'holds', from: 'iter_engine', to: 'account', desc: 'the engine has the credential locally', keys: { token_envar: '' } },
    { edge: 'of', from: 'account', to: 'provider', desc: 'the provider the account dispatches to', keys: {} },
    { edge: 'member', from: 'user', to: 'project', desc: 'the user may see / act on the project', keys: { role: 'user' } },
    { edge: 'owns', from: 'user', to: 'iter_engine', desc: 'whose token the engine uses', keys: {} },
    { edge: 'runs', from: 'agent', to: 'project', desc: 'the agent is enabled on the project (its settings override the agent)', keys: {} },
    { edge: 'handles', from: 'agent', to: 'workitem_type', desc: 'the agent handles items in this state', keys: {} },
    { edge: 'allows', from: 'project', to: 'workitem_type', desc: 'per-state policy for the project', keys: {} },
    { edge: 'uses', from: 'agent_tools', to: 'agent', desc: 'tooling the agent can look up', keys: {} },
    { edge: 'hosts', from: 'iter_data', to: 'project', desc: 'display only', keys: {} },
    { edge: 'hosts', from: 'iter_data', to: 'iter_engine', desc: 'display only', keys: {} },
  ];
  const EDGE_COLOR = { serves: '#4da3ff', bills: '#ec4899', holds: '#60a5fa', of: '#22d3ee', member: '#f59e0b', owns: '#fbbf24', runs: '#2dd4bf', handles: '#94a3b8', allows: '#a78bfa', uses: '#5eead4', hosts: '#64748b' };
  const typeFor = (ft, tt) => (EDGE_TYPES.find((d) => d.from === ft && d.to === tt) || {}).edge || null;
  const edgeDef = (e, ft, tt) => EDGE_TYPES.find((d) => d.edge === e && (!ft || d.from === ft) && (!tt || d.to === tt));
  const PH = '_deactivated';
  const typeOfId = (id) => String(id).split(':')[0];

  let S = { el: null, ctx: null, cy: null, data: null, byId: new Map(), sel: null, hide: new Set(), layout: 'columns', handles: null, draw: null, timer: null, keyBound: false, pending: null };
  const admin = () => !!(S.ctx && S.ctx.isAdmin);
  const api = (p, o) => S.ctx.api(p, o);
  const toast = (m, o) => K().toast(m, o);
  const errMsg = (e) => (e && e.body && e.body.error) || (e && e.message) || String(e);
  const $ = (id) => S.el.querySelector('#s-' + id);
  const nodeName = (id) => { const n = S.byId.get(id); if (!n) return id; return n.placeholder ? `deactivated ${(TYPES[n.type] || {}).label || n.type}` : (n.name || id); };

  const TEMPLATE = `
<div class="g-bar">
  <div class="g-row g-toprow">
    <div class="g-group"><b class="s-title">Settings graph</b><span class="s-sub" id="s-sub"></span></div>
    <div class="g-group">
      <div class="g-seg" id="s-layouts" role="group" aria-label="Layout">
        <button data-layout="columns" title="One column per kind of thing, left to right: who → engines → accounts and agents → projects → states">Columns</button>
        <button data-layout="flow" title="Left to right, following the arrows (dagre)">Flow</button>
        <button data-layout="force" title="Force-directed">Force</button>
      </div>
    </div>
    <div class="g-group g-searchgrp"><div class="g-searchbox"><input type="search" id="s-search" placeholder="Find a node or tag" autocomplete="off" spellcheck="false" aria-label="Find a node or edge tag"><kbd class="g-kbdhint">/</kbd></div></div>
    <div class="g-group g-tools">
      <button class="g-btn" id="s-fit" title="Fit (F)">Fit</button>
      <button class="g-btn" id="s-relayout" title="Run the layout again (R)">Re-layout</button>
      <button class="g-btn g-icon" id="s-help" title="Shortcuts and how it works (?)" aria-label="Help">?</button>
    </div>
  </div>
  <div class="g-row g-chiprow"><span class="g-lbl">Show</span><div class="g-chips" id="s-chips"></div>
    <label class="g-chk" title="Show each type's deactivated placeholder (drop an edge end on it to switch the edge off)"><input type="checkbox" id="s-ph"> placeholders</label>
    <label class="g-chk" title="Show switched-off edges"><input type="checkbox" id="s-inactive" checked> inactive edges</label></div>
  <div class="g-row g-editrow"><span class="g-summary" id="s-summary"></span><span class="g-edit" id="s-editbar"></span></div>
</div>
<div class="g-main">
  <div id="s-cywrap" class="s-cywrap"><div id="s-cy" class="s-cy"></div><div id="s-banner" class="g-floatbanner hidden"></div><div id="s-empty" class="g-empty hidden"></div></div>
  <aside id="s-detail" class="g-detail hidden" aria-label="Details"></aside>
</div>
<div class="g-legend" id="s-legend"></div>`;

  // ------------------------------------------------------------------ load
  async function load(keep) {
    let g;
    try { g = await api('/api/settings/graph'); }
    catch (e) {
      $('empty').classList.remove('hidden');
      $('empty').innerHTML = e.status === 404 ? 'This server has no settings graph yet (GET /api/settings/graph is missing).' : `Could not load the settings graph: ${esc(errMsg(e))}`;
      return false;
    }
    const nodes = (g.nodes || []).filter((n) => n && n.id);
    const ids = new Set(nodes.map((n) => n.id));
    // placeholders the server leaves virtual: make sure every type has one to drop on
    Object.keys(TYPES).forEach((t) => { const id = `${t}:${PH}`; if (t !== 'iter_data' && !ids.has(id)) { nodes.push({ id, type: t, name: PH, placeholder: true, deactivated: true, settings: {}, summary: 'placeholder', virtual: true }); ids.add(id); } });
    nodes.forEach((n) => { n.type = n.type || typeOfId(n.id); n.name = n.name || String(n.id).slice(n.type.length + 1); if (n.name === PH) { n.placeholder = true; } });
    const edges = (g.edges || []).filter((e) => e && e.id && ids.has(e.from) && ids.has(e.to));
    S.data = { nodes, edges };
    S.byId = new Map(nodes.map((n) => [n.id, n]));
    draw(keep);
    return true;
  }

  // ------------------------------------------------------------------ draw
  function ensureCy() {
    if (S.cy) return S.cy;
    if (window.IterGraph && IterGraph.registerLayouts) IterGraph.registerLayouts();
    S.cy = cytoscape({
      container: $('cy'), minZoom: 0.05, maxZoom: 3, boxSelectionEnabled: false, selectionType: 'single', userZoomingEnabled: false,
      style: [
        { selector: 'node', style: { 'background-color': 'data(color)', 'border-color': 'data(border)', 'border-width': 1.5, shape: 'data(shape)', width: 'data(w)', height: 'data(h)',
          label: 'data(label)', 'font-size': 12, 'font-family': 'ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif', color: '#d8dce3', 'text-valign': 'bottom', 'text-margin-y': 4,
          'text-wrap': 'wrap', 'text-max-width': '140px', 'text-background-color': '#14161a', 'text-background-opacity': 0.8, 'text-background-padding': '1px', 'text-background-shape': 'roundrectangle', 'min-zoomed-font-size': 5 } },
        { selector: 'node.project, node.iter_engine, node.iter_data', style: { 'font-weight': 600, 'font-size': 13 } },
        { selector: 'node.ph', style: { 'background-color': '#22262d', 'border-color': '#5b6270', 'border-style': 'dashed', 'border-width': 2, color: '#7c8594', 'font-style': 'italic', 'font-size': 11 } },
        { selector: 'node.off', style: { 'background-color': '#3a3f48', 'border-color': '#5b6270', color: '#8b93a1', opacity: 0.75 } },
        { selector: 'edge', style: { width: 1.8, 'line-color': 'data(color)', 'target-arrow-color': 'data(color)', 'target-arrow-shape': 'triangle', 'arrow-scale': 0.85, 'curve-style': 'bezier', opacity: 0.8,
          label: 'data(label)', 'font-size': 10.5, color: '#c3c9d3', 'text-background-color': '#14161a', 'text-background-opacity': 0.9, 'text-background-padding': '2px', 'text-background-shape': 'roundrectangle',
          'text-rotation': 'autorotate', 'min-zoomed-font-size': 7 } },
        { selector: 'edge.arc', style: { 'curve-style': 'unbundled-bezier', 'control-point-distances': [-90], 'control-point-weights': [0.5] } },
        // an untagged edge's type label ("serves", "runs") stays hidden at overview zoom, where many converge on one node
        // and the labels stack; it shows when zoomed in (≥ ~1.5×), on hover, and for the selection and its node's edges (.lbl)
        { selector: 'edge.typed', style: { 'text-opacity': 0, 'text-background-opacity': 0 } },
        { selector: 'edge.typed.lbl, edge.typed:selected, edge.typed.zin', style: { 'text-opacity': 1, 'text-background-opacity': 0.9 } },
        { selector: 'edge.typed.lbl, edge.typed:selected', style: { 'z-index': 20 } },
        { selector: 'edge.tagged', style: { color: '#f1f5f9', 'font-weight': 600, 'text-border-width': 1, 'text-border-color': 'data(color)', 'text-border-opacity': 0.5 } },
        { selector: 'edge.inactive', style: { 'line-color': '#5b6270', 'target-arrow-color': '#5b6270', 'line-style': 'dashed', opacity: 0.6, color: '#7c8594' } },
        { selector: 'edge.hosts', style: { 'line-style': 'dotted', opacity: 0.4, width: 1.2 } },
        { selector: 'edge.handles, edge.allows, edge.uses', style: { width: 1.2, opacity: 0.55 } },
        { selector: '.faded', style: { opacity: 0.1 } },
        { selector: '.selfade', style: { opacity: 0.15 } },
        { selector: 'node.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.45, 'overlay-padding': 7 } },
        { selector: 'edge.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.4, 'overlay-padding': 5 } },
        { selector: 'node:selected', style: { 'border-width': 4, 'border-color': '#f8fafc', 'overlay-color': '#4da3ff', 'overlay-opacity': 0.18, 'overlay-padding': 6 } },
        { selector: 'edge:selected', style: { width: 4, opacity: 1, 'z-index': 30, 'overlay-color': '#4da3ff', 'overlay-opacity': 0.15, 'overlay-padding': 4 } },
        { selector: 'node.g-flash', style: { 'overlay-color': '#4ade80', 'overlay-opacity': 0.45, 'overlay-padding': 10 } },
      ],
    });
    wireCy(S.cy);
    return S.cy;
  }
  const isHelper = (el) => el.hasClass('kit-ep') || el.id().startsWith('__');
  function visibleNode(n) {
    if (S.hide.has(n.type)) return false;
    if (n.placeholder && !$('ph').checked) {
      // a placeholder that holds an edge is always shown, so the inactive edge has somewhere to end
      if (!S.data.edges.some((e) => (e.from === n.id || e.to === n.id) && $('inactive').checked)) return false;
    }
    return true;
  }
  function draw(keep) {
    const cy = ensureCy();
    const snap = keep ? new Map(cy.nodes().filter((n) => !isHelper(n)).map((n) => [n.id(), Object.assign({}, n.position())])) : null;
    const vp = keep ? { zoom: cy.zoom(), pan: Object.assign({}, cy.pan()) } : null;
    const nodes = S.data.nodes.filter(visibleNode);
    const ids = new Set(nodes.map((n) => n.id));
    const showInactive = $('inactive').checked;
    const els = nodes.map((n) => {
      const t = TYPES[n.type] || TYPES.workitem_type;
      const label = n.placeholder ? `⊘ deactivated ${t.label.toLowerCase()}` : (n.name + (n.deactivated ? ' (off)' : ''));
      return { group: 'nodes', position: { x: Math.random() * 600, y: Math.random() * 400 }, classes: [n.type, n.placeholder ? 'ph' : '', n.deactivated && !n.placeholder ? 'off' : ''].join(' '),
        data: { id: n.id, label, color: t.color, border: t.border, shape: t.shape, w: n.type === 'project' && !n.placeholder ? t.w : t.w * (n.placeholder ? 0.85 : 1), h: t.h * (n.placeholder ? 0.85 : 1) } };
    });
    S.data.edges.forEach((e) => {
      if (!ids.has(e.from) || !ids.has(e.to)) return;
      const active = e.active !== false;
      if (!active && !showInactive) return;
      els.push({ group: 'edges', classes: [e.type, active ? '' : 'inactive', e.tag ? 'tagged' : 'typed'].join(' '),
        data: { id: 'se:' + e.id, source: e.from, target: e.to, color: EDGE_COLOR[e.type] || '#94a3b8', label: e.tag ? e.tag : (['handles', 'allows', 'uses', 'hosts'].includes(e.type) ? '' : e.type), eid: e.id } });
    });
    cy.batch(() => { cy.elements().remove(); cy.add(els); });
    S.zin = null; zoomLabels();
    $('empty').classList.toggle('hidden', nodes.length > 0);
    if (!nodes.length) $('empty').textContent = 'Nothing to show: every type is switched off above.';
    let placed = 0;
    if (snap) cy.nodes().forEach((n) => { const p = snap.get(n.id()); if (p) { n.position(p); placed++; } });
    if (snap && placed >= cy.nodes().length * 0.7) {
      cy.nodes().filter((n) => !snap.has(n.id())).forEach((n) => { const nb = n.neighborhood('node').filter((x) => snap.has(x.id()))[0]; const at = nb ? nb.position() : { x: 0, y: 0 }; n.position({ x: at.x + 80, y: at.y + 50 + Math.random() * 40 }); });
      cy.viewport(vp);
      bowEdges();
    } else layout();
    renderChips(); renderSummary(); renderBar(); applySearch();
    if (S.sel) reselect();
  }
  /** An edge that jumps over a column runs straight through the nodes of the column between
   *  (engine → project crosses the accounts): bow it so its line and tag stay readable. */
  /** Untagged edges' type labels show once zoomed in far enough that they no longer stack (.zin). */
  function zoomLabels() {
    const cy = S.cy; const z = cy.zoom() >= 1.6;
    if (z === S.zin) return;
    S.zin = z; cy.batch(() => cy.edges('.typed').toggleClass('zin', z));
  }
  function bowEdges() {
    const cy = S.cy;
    cy.batch(() => cy.edges().forEach((e) => {
      if (isHelper(e)) return;
      const dx = Math.abs(e.source().position('x') - e.target().position('x'));
      const dy = Math.abs(e.source().position('y') - e.target().position('y'));
      e.toggleClass('arc', S.layout === 'columns' && dx > 300 && dy < 120);
    }));
  }
  function layout() {
    const cy = S.cy; cy.resize();
    if (cy.nodes().empty()) return;
    if (S.layout === 'flow') cy.layout({ name: 'dagre', rankDir: 'LR', nodeSep: 18, rankSep: 140, nodeDimensionsIncludeLabels: true, fit: false, animate: false }).run();
    else if (S.layout === 'force') cy.layout({ name: 'fcose', quality: 'default', randomize: true, animate: false, nodeDimensionsIncludeLabels: true, idealEdgeLength: () => 110, nodeRepulsion: () => 9000, fit: false }).run();
    else {
      // columns: one per group of types, rows sorted by name, placeholders at the bottom of their type
      const cols = new Map();
      cy.nodes().forEach((n) => { const d = S.byId.get(n.id()); const t = TYPES[d.type] || {}; const c = t.col ?? 4; if (!cols.has(c)) cols.set(c, []); cols.get(c).push(d); });
      const COLW = 270; const ROWH = 62; const GAP = 34;
      [...cols.keys()].sort((a, b) => a - b).forEach((c) => {
        const list = cols.get(c).sort((a, b) => COL_ORDER.indexOf(a.type) - COL_ORDER.indexOf(b.type) || (a.placeholder ? 1 : 0) - (b.placeholder ? 1 : 0) || String(a.name).localeCompare(String(b.name), undefined, { numeric: true }));
        let y = 0; let prevType = null; const pos = [];
        list.forEach((d) => { if (prevType && prevType !== d.type) y += GAP; pos.push([d.id, y]); y += ROWH; prevType = d.type; });
        const off = y / 2;
        pos.forEach(([id, yy]) => cy.getElementById(id).position({ x: c * COLW, y: yy - off }));
      });
    }
    bowEdges();
    cy.fit(cy.elements().not('.kit-ep'), 30); if (cy.zoom() > 1.3) { cy.zoom(1.3); cy.center(); }
  }

  // ------------------------------------------------------------------ chrome
  function renderChips() {
    const counts = {}; S.data.nodes.forEach((n) => { if (!n.placeholder) counts[n.type] = (counts[n.type] || 0) + 1; });
    $('chips').innerHTML = TYPE_ORDER.map((k) => { const t = TYPES[k]; const on = !S.hide.has(k);
      return `<button type="button" class="g-chip ${on ? 'on' : ''} ${counts[k] ? '' : 'zero'}" data-type="${k}" aria-pressed="${on}" title="${esc(t.long)} — click to ${on ? 'hide' : 'show'}; alt-click to show only this type"><span class="g-chipsw s-${k}" style="--c:${t.color};--b:${t.border}"></span>${esc(t.label)}<em>${counts[k] || 0}</em></button>`; }).join('');
    S.el.querySelectorAll('#s-layouts button').forEach((b) => b.classList.toggle('on', b.dataset.layout === S.layout));
  }
  function renderSummary() {
    const n = S.data.nodes.filter((x) => !x.placeholder).length; const e = S.data.edges.length; const off = S.data.edges.filter((x) => x.active === false).length;
    $('summary').textContent = `${n} nodes · ${e} edges${off ? ` (${off} inactive)` : ''}`;
    $('sub').textContent = admin() ? 'every setting is a node or an edge' : 'read only — an admin edits';
  }
  function renderBar() {
    const bar = $('editbar');
    if (!admin()) { bar.innerHTML = '<span class="g-ro">read only (admin edits)</span>'; return; }
    const sel = S.sel; const clip = K().clip.settings;
    bar.innerHTML = '<button type="button" class="g-btn g-primary" data-ed="new" title="A new engine, project, account, agent, user… (N)">+ Node</button>'
      + `<button type="button" class="g-btn" data-ed="connect" ${sel && sel.kind === 'node' ? '' : 'disabled'} title="Draw an edge from the selected node; its type follows from the two ends (C)">Connect</button>`
      + `<button type="button" class="g-btn" data-ed="paste" ${clip && sel && sel.kind === 'node' ? '' : 'disabled'} title="${clip ? 'Paste the copied ' + esc(clip.type) + ' edge (with its settings) onto the selected node (⌘V)' : 'Copy an edge first (⌘C)'}">Paste edge</button>`;
    bar.querySelectorAll('[data-ed]').forEach((b) => { b.onclick = () => { const n = S.sel && S.sel.kind === 'node' ? S.byId.get(S.sel.id) : null; ({ new: () => newNode(), connect: () => n && startDraw(n), paste: () => n && pasteEdge(n) })[b.dataset.ed](); }; });
  }
  function renderLegend() {
    const nodes = TYPE_ORDER.map((k) => `<div class="item"><span class="g-chipsw s-${k}" style="--c:${TYPES[k].color};--b:${TYPES[k].border}"></span>${esc(TYPES[k].long)}</div>`).join('')
      + '<div class="item"><span class="g-fsw dashed" style="border-color:#5b6270;background:#22262d"></span>deactivated placeholder</div>';
    const seen = new Set();
    const edges = EDGE_TYPES.filter((d) => !seen.has(d.edge) && seen.add(d.edge)).map((d) => `<div class="item"><svg width="30" height="8" aria-hidden="true"><line x1="1" y1="4" x2="23" y2="4" stroke="${EDGE_COLOR[d.edge]}" stroke-width="2"/><polygon points="23,0 30,4 23,8" fill="${EDGE_COLOR[d.edge]}"/></svg><b>${esc(d.edge)}</b>&nbsp;${esc(d.from)} → ${esc(EDGE_TYPES.filter((x) => x.edge === d.edge).map((x) => x.to).join(' / '))}</div>`).join('')
      + '<div class="item"><svg width="30" height="8" aria-hidden="true"><line x1="1" y1="4" x2="23" y2="4" stroke="#5b6270" stroke-width="2" stroke-dasharray="5 3"/><polygon points="23,0 30,4 23,8" fill="#5b6270"/></svg>inactive (switched off, or one end on a placeholder)</div>';
    $('legend').innerHTML = `<button type="button" class="legendtoggle" id="s-legendToggle">Legend</button><div class="legendbody"><div class="row"><span class="lbl">Nodes</span>${nodes}</div><div class="row"><span class="lbl">Edges</span>${edges}</div>
      <div class="row note"><span class="lbl">Note</span><span>A tagged edge always shows its tag; an untagged edge shows its type when you hover it or one of its nodes, select it, or zoom in. Drag an edge's end onto a <i>deactivated</i> placeholder to switch it off and keep its settings; drag it back to switch it on.</span></div></div>`;
    let open = false; try { open = localStorage.getItem('iter5.settings.legendOpen') === '1'; } catch (e) { /* storage blocked */ }
    const apply = () => { $('legend').classList.toggle('collapsed', !open); $('legendToggle').textContent = open ? 'Legend ▾' : 'Legend ▸'; if (S.cy) S.cy.resize(); };
    $('legendToggle').onclick = () => { open = !open; try { localStorage.setItem('iter5.settings.legendOpen', open ? '1' : '0'); } catch (e) { /* storage blocked */ } apply(); };
    apply();
  }

  // ------------------------------------------------------------------ selection + detail
  function select(sel) {
    S.sel = sel;
    const cy = S.cy;
    cy.elements().removeClass('selfade faded');
    if (!sel) { cy.edges('.lbl').removeClass('lbl'); cy.elements(':selected').unselect(); $('detail').classList.add('hidden'); cy.resize(); renderBar(); return; }
    reselect();
    $('detail').classList.remove('hidden'); cy.resize();
    // the detail pane narrows the canvas: keep the selection on screen
    const el = cy.getElementById(sel.kind === 'edge' ? 'se:' + sel.id : sel.id);
    if (el.nonempty()) {
      const p = el.isEdge() ? el.renderedMidpoint() : el.renderedPosition(); const w = cy.width(); const h = cy.height(); const m = 60;
      const dx = p.x < m ? m - p.x : p.x > w - m ? (w - m) - p.x : 0; const dy = p.y < m ? m - p.y : p.y > h - m ? (h - m) - p.y : 0;
      if ((dx || dy) && Number.isFinite(dx + dy)) cy.animate({ panBy: { x: dx, y: dy } }, { duration: 200 });
    }
    renderDetail(); renderBar();
  }
  function reselect() {
    const cy = S.cy; const id = S.sel.kind === 'edge' ? 'se:' + S.sel.id : S.sel.id;
    const el = cy.getElementById(id);
    if (el.empty()) return;
    cy.elements(':selected').not(el).unselect();
    if (!el.selected()) el.select();
    const keep = el.isNode() ? el.closedNeighborhood() : el.union(el.connectedNodes());
    cy.elements().not(keep).not('.kit-ep').addClass('selfade');
    cy.edges('.lbl').removeClass('lbl');
    (el.isNode() ? el.connectedEdges() : el).addClass('lbl');
  }
  const settingsTable = (o) => { const ks = Object.keys(o || {}); if (!ks.length) return '<p class="muted">No settings.</p>';
    return `<dl class="g-facts">${ks.map((k) => `<dt>${esc(k)}</dt><dd>${esc(typeof o[k] === 'object' ? JSON.stringify(o[k]) : String(o[k]).length > 160 ? String(o[k]).slice(0, 160) + '…' : o[k])}</dd>`).join('')}</dl>`; };
  const nodeLink = (id) => { const n = S.byId.get(id); const t = TYPES[n ? n.type : typeOfId(id)] || TYPES.workitem_type; return `<span class="dot" style="background:${n && n.placeholder ? '#5b6270' : t.color}"></span><a class="node" data-node="${esc(id)}">${esc(nodeName(id))}</a>`; };
  function renderDetail() {
    const box = $('detail'); const sel = S.sel;
    let h = '<button class="close" title="Close (Esc)" aria-label="Close">×</button>';
    const btn = (k, t, cls, tip) => `<button type="button" class="g-btn ${cls || ''}" data-act="${k}" title="${esc(tip || '')}">${esc(t)}</button>`;
    if (sel.kind === 'node') {
      const n = S.byId.get(sel.id); if (!n) return;
      const t = TYPES[n.type] || {};
      h += `<h2>${esc(nodeName(n.id))}</h2><div class="g-badges"><span class="badge" style="background:${n.placeholder ? '#3a3f48' : t.color}">${esc(t.label || n.type)}</span>${n.placeholder ? '<span class="g-pill">placeholder</span>' : n.deactivated ? '<span class="g-pill">off</span>' : ''}</div>`;
      if (n.summary && !n.placeholder) h += `<div class="simple">${esc(n.summary)}</div>`;
      if (n.placeholder) h += `<div class="simple missing">Drop an edge's end here to switch that edge off; its settings are kept. It cannot be deleted.</div>`;
      h += `<div class="actions">${btn('configure', admin() && !n.placeholder ? 'Configure…' : 'View settings…', 'g-primary')}${admin() && !n.placeholder ? btn('connect', 'Connect', '', 'draw an edge from here (C)') : ''}${admin() && K().clip.settings ? btn('paste', 'Paste edge', '', '⌘V') : ''}${n.type === 'project' && !n.placeholder && S.ctx.openProject ? btn('open', 'Open project', '', 'its work queue and project graph') : ''}${admin() && !n.placeholder && n.type !== 'iter_data' ? btn('delete', 'Delete…', 'g-danger') : ''}</div>`;
      h += `<h4>Id</h4><span class="path">${esc(n.id)}</span>`;
      if (!n.placeholder) h += `<h4>Settings</h4>${settingsTable(n.settings)}`;
      const out = S.data.edges.filter((e) => e.from === n.id); const inn = S.data.edges.filter((e) => e.to === n.id);
      const list = (arr, other, title) => arr.length ? `<h4>${title} (${arr.length})</h4><ul>${arr.map((e) => `<li><span class="g-eswatch" style="background:${e.active === false ? '#5b6270' : EDGE_COLOR[e.type]}"></span><a class="edge g-elink" data-edge="${esc(e.id)}">${esc(e.type)}${e.tag ? ' · ' + esc(e.tag) : ''}</a> ${nodeLink(other(e))}${e.active === false ? ' <span class="muted">(inactive)</span>' : ''}</li>`).join('')}</ul>` : '';
      h += list(out, (e) => e.to, 'Edges out') + list(inn, (e) => e.from, 'Edges in');
    } else {
      const e = S.data.edges.find((x) => x.id === sel.id); if (!e) return;
      const d = edgeDef(e.type, typeOfId(e.from), typeOfId(e.to)) || {};
      h += `<h2><span class="g-eswatch big" style="background:${EDGE_COLOR[e.type] || '#888'}"></span>${esc(e.type)}${e.tag ? ` <span class="s-tag">${esc(e.tag)}</span>` : ''}</h2>
        <div class="g-badges">${e.active === false ? `<span class="g-pill bad">inactive — ${[e.from, e.to].some((x) => String(x).endsWith(':' + PH)) ? 'an end is on a deactivated placeholder' : 'switched off'}</span>` : '<span class="g-pill ok">active</span>'}</div>
        <div class="g-edgeends"><div>${nodeLink(e.from)}</div><div class="g-edgearrow">${esc(e.type)} →</div><div>${nodeLink(e.to)}</div></div>
        <p class="muted">${esc(d.desc || '')}</p>
        <div class="actions">${btn('configure', admin() ? 'Configure…' : 'View…', 'g-primary')}${btn('copy', 'Copy edge', '', '⌘C, then select a node and ⌘V')}${admin() ? btn('tag', 'Tag…', '', 'a short name, shown on the edge') + btn('toggle', e.enabled === false ? 'Switch on' : 'Switch off', '', 'keeps the edge and its settings') + btn('delete', 'Delete…', 'g-danger') : ''}</div>
        <h4>Settings</h4>${settingsTable(e.settings)}
        <dl class="g-facts"><dt>id</dt><dd class="g-id">${esc(e.id)}</dd>${e.created ? `<dt>created</dt><dd>${esc(e.created)}</dd>` : ''}${e.updated ? `<dt>updated</dt><dd>${esc(e.updated)}</dd>` : ''}</dl>
        ${admin() ? '<p class="muted">Select the edge and drag a round handle at either end onto another node to move it; onto a deactivated placeholder to switch it off.</p>' : ''}`;
    }
    box.innerHTML = h;
    box.querySelector('.close').onclick = () => select(null);
    box.querySelectorAll('a.node').forEach((a) => { a.onclick = () => focus(a.dataset.node); });
    box.querySelectorAll('a.edge').forEach((a) => { a.onclick = () => select({ kind: 'edge', id: a.dataset.edge }); });
    box.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => act(b.dataset.act); });
  }
  function act(k) {
    const sel = S.sel; if (!sel) return;
    if (sel.kind === 'node') {
      const n = S.byId.get(sel.id);
      ({ configure: () => configureNode(n), connect: () => startDraw(n), paste: () => pasteEdge(n), delete: () => deleteNode(n), open: () => S.ctx.openProject(n.name) })[k]();
    } else {
      const e = S.data.edges.find((x) => x.id === sel.id);
      ({ configure: () => configureEdge(e), copy: () => copyEdge(e), tag: () => tagEdge(e), toggle: () => toggleEdge(e), delete: () => deleteEdge(e) })[k]();
    }
  }
  function focus(id) {
    if (!S.cy) { S.pending = id; return false; }
    let el = S.cy.getElementById(id);
    const n = S.byId.get(id);
    if (el.empty() && n) { S.hide.delete(n.type); if (n.placeholder) $('ph').checked = true; draw(true); el = S.cy.getElementById(id); }
    if (el.empty()) return false;
    select({ kind: 'node', id });
    S.cy.animate({ center: { eles: el }, zoom: Math.max(S.cy.zoom(), 0.9) }, { duration: 250 });
    el.addClass('g-flash'); setTimeout(() => el.removeClass('g-flash'), 1500);
    return true;
  }

  // ------------------------------------------------------------------ cytoscape events
  function wireCy(cy) {
    const host = () => $('cywrap');
    const at = (e) => e.renderedPosition || (e.target.isEdge && e.target.isEdge() ? e.target.renderedMidpoint() : e.target.renderedPosition());
    const edgeOf = (el) => S.data.edges.find((x) => 'se:' + x.id === el.id());
    cy.on('tap', 'node', (e) => { if (isHelper(e.target)) return; if (S.draw) { finishDraw(e.target.id()); return; } select({ kind: 'node', id: e.target.id() }); });
    cy.on('tap', 'edge', (e) => { if (isHelper(e.target)) return; const r = edgeOf(e.target); if (r) select({ kind: 'edge', id: r.id }); });
    cy.on('tap', (e) => { if (e.target === cy) { if (S.draw) cancelDraw(); else select(null); } });
    cy.on('dbltap', 'node', (e) => { if (!isHelper(e.target)) configureNode(S.byId.get(e.target.id())); });
    cy.on('dbltap', 'edge', (e) => { const r = edgeOf(e.target); if (r) configureEdge(r); });
    const nodeMenu = (e) => { if (isHelper(e.target)) return; const n = S.byId.get(e.target.id()); if (!n) return; select({ kind: 'node', id: n.id });
      const items = admin() && !n.placeholder ? [{ key: 'configure', label: 'Configure…', kbd: 'E' }, { key: 'connect', label: 'Connect from here', kbd: 'C' }, { key: 'paste', label: 'Paste edge here', kbd: K().MOD + 'V', disabled: !K().clip.settings }, ...(n.type === 'project' && S.ctx.openProject ? [{ key: 'open', label: 'Open project' }] : []), { sep: true }, { key: 'delete', label: 'Delete…', danger: true, disabled: n.type === 'iter_data' }]
        : [{ key: 'configure', label: 'View settings…' }, ...(admin() && K().clip.settings ? [{ key: 'paste', label: 'Paste edge here', kbd: K().MOD + 'V' }] : [])];
      K().menu(host(), at(e), `${(TYPES[n.type] || {}).label || n.type} · ${nodeName(n.id)}`, items, (k) => act(k)); };
    const edgeMenu = (e) => { const r = edgeOf(e.target); if (!r) return; select({ kind: 'edge', id: r.id });
      const items = admin() ? [{ key: 'configure', label: 'Configure…', kbd: 'E' }, { key: 'copy', label: 'Copy edge', kbd: K().MOD + 'C' }, { key: 'tag', label: 'Tag…' }, { key: 'toggle', label: r.enabled === false ? 'Switch on' : 'Switch off' }, { sep: true }, { key: 'delete', label: 'Delete…', kbd: 'Del', danger: true }]
        : [{ key: 'configure', label: 'View…' }, { key: 'copy', label: 'Copy edge' }];
      K().menu(host(), at(e), `${r.type}${r.tag ? ' · ' + r.tag : ''}`, items, (k) => act(k)); };
    cy.on('cxttap taphold', 'node', nodeMenu);
    cy.on('cxttap taphold', 'edge', edgeMenu);
    cy.on('cxttap', (e) => { if (e.target === cy && admin()) K().menu(host(), at(e), 'Settings graph', [{ key: 'new', label: 'New node…', kbd: 'N' }, { key: 'help', label: 'Shortcuts…', kbd: '?' }], (k) => (k === 'new' ? newNode() : openHelp())); });
    $('cywrap').addEventListener('contextmenu', (ev) => ev.preventDefault());
    cy.on('mouseover', 'node', (e) => { if (S.sel || isHelper(e.target) || S.draw) return; const hood = e.target.closedNeighborhood(); cy.elements().not(hood).not('.kit-ep').addClass('faded'); hood.edges().addClass('lbl'); });
    cy.on('mouseout', 'node', () => { cy.elements().removeClass('faded'); if (!S.sel) cy.edges('.lbl').removeClass('lbl'); });
    // never restyle inside cytoscape's own zoom notification (re-entrant restyle during a viewport change crashes the renderer): defer it
    cy.on('zoom', () => { clearTimeout(S.zinTimer); S.zinTimer = setTimeout(zoomLabels, 60); });
    cy.on('mouseover', 'edge', (e) => { if (!isHelper(e.target)) e.target.addClass('lbl'); });
    cy.on('mouseout', 'edge', (e) => { if (S.sel && S.sel.kind === 'node' && e.target.connectedNodes().some((n) => n.id() === S.sel.id)) return; if (!(S.sel && S.sel.kind === 'edge' && e.target.id() === 'se:' + S.sel.id)) e.target.removeClass('lbl'); });
    S.handles = K().endpointHandles(cy, {
      canEdit: (edge) => admin() && !!edgeOf(edge),
      accept: (edge, end, n) => {
        const r = edgeOf(edge); const nn = S.byId.get(n.id()); if (!r || !nn) return 'Drop it on a node.';
        const ft = end === 'source' ? nn.type : typeOfId(r.from); const tt = end === 'target' ? nn.type : typeOfId(r.to);
        return edgeDef(r.type, ft, tt) ? true : `A ${r.type} edge joins ${EDGE_TYPES.filter((d) => d.edge === r.type).map((d) => d.from + ' → ' + d.to).join(' or ')}; that end cannot go on a ${nn.type}.`;
      },
      onDrop: (edge, end, n) => { const r = edgeOf(edge); if (r) patchEdge(r, end === 'source' ? { from: n.id() } : { to: n.id() }, (S.byId.get(n.id()) || {}).placeholder ? 'Edge switched off (moved onto the placeholder); its settings are kept.' : 'Edge moved'); },
    });
    const wheelPx = (ev, d) => d * (ev.deltaMode === 1 ? 16 : ev.deltaMode === 2 ? $('cy').clientHeight : 1);
    $('cy').addEventListener('wheel', (ev) => {
      ev.preventDefault(); const box = $('cy').getBoundingClientRect(); const p = { x: ev.clientX - box.left, y: ev.clientY - box.top };
      if (ev.ctrlKey || ev.metaKey) { const d = Math.max(-25, Math.min(25, wheelPx(ev, ev.deltaY))); cy.zoom({ level: Math.min(cy.maxZoom(), Math.max(cy.minZoom(), cy.zoom() * Math.exp(-d * 0.01))), renderedPosition: p }); }
      else { let dx = wheelPx(ev, ev.deltaX); let dy = wheelPx(ev, ev.deltaY); if (ev.shiftKey && !dx) { dx = dy; dy = 0; } cy.panBy({ x: -dx, y: -dy }); }
    }, { passive: false });
  }

  // ------------------------------------------------------------------ edits
  async function reload(focusSel) { await load(true); if (focusSel) { S.sel = focusSel; if (S.cy.getElementById(focusSel.kind === 'edge' ? 'se:' + focusSel.id : focusSel.id).nonempty()) select(focusSel); else select(null); } else if (S.sel) { const ok = S.sel.kind === 'node' ? S.byId.has(S.sel.id) : S.data.edges.some((e) => e.id === S.sel.id); select(ok ? S.sel : null); } }
  function settingsFields(obj, defaults) {
    const keys = Object.keys(Object.assign({}, defaults || {}, obj || {}));
    return keys.map((k) => ({ key: k, value: obj && k in obj ? obj[k] : defaults[k], removable: true, type: k === 'promptbody' || k === 'body' ? 'markdown' : undefined, rows: k === 'promptbody' || k === 'body' ? 16 : undefined, isNew: !(obj && k in obj) }));
  }
  async function configureNode(n) {
    if (!n) return;
    let rec = n;
    try { if (!n.virtual) rec = Object.assign({}, n, await api('/api/settings/nodes/' + enc(n.id))); } catch (e) { /* the graph's copy will do */ }
    const ro = !admin() || n.placeholder;
    return K().configure({ title: `${(TYPES[n.type] || {}).label || n.type} — ${nodeName(n.id)}`, sub: rec.summary || '', readOnly: ro, allowAdd: !ro,
      meta: [['id', `<span class="g-mono">${esc(n.id)}</span>`], ['type', esc(n.type)], ['edges', String(S.data.edges.filter((e) => e.from === n.id || e.to === n.id).length)]],
      note: ro ? (n.placeholder ? 'A placeholder has no settings: edges that end here are switched off.' : 'Read only — an admin edits.') : 'Each key is one setting. × removes a key; + Add setting adds one.',
      fields: settingsFields(rec.settings || {}, {}),
      onSave: async ({ changed, removed }) => {
        const patch = Object.assign({}, changed); removed.forEach((k) => { patch[k] = null; });
        if (!Object.keys(patch).length) return true;
        try { await api('/api/settings/nodes/' + enc(n.id), { method: 'PATCH', body: JSON.stringify({ settings: patch }) }); } catch (e) { throw new Error(errMsg(e)); }
        toast('Saved', { kind: 'ok' }); reload({ kind: 'node', id: n.id }); if (S.ctx.onChange) S.ctx.onChange(); return true;
      } });
  }
  async function configureEdge(e) {
    if (!e) return;
    const ro = !admin();
    const ft = typeOfId(e.from); const tt = typeOfId(e.to);
    const d = edgeDef(e.type, ft, tt) || {};
    const opts = (type) => S.data.nodes.filter((n) => n.type === type).map((n) => ({ value: n.id, label: nodeName(n.id) }));
    const fields = [
      { key: '__tag', label: 'tag', value: e.tag || '', type: 'text', group: 'Edge', help: 'a short name shown on the edge, e.g. pdy_default' },
      { key: '__enabled', label: 'switched on', value: e.enabled !== false, type: 'bool', group: 'Edge', help: 'off keeps the edge and its settings but it stops counting' },
      { key: '__from', label: 'from', value: e.from, type: 'select', options: opts(ft), group: 'Edge' },
      { key: '__to', label: 'to', value: e.to, type: 'select', options: opts(tt), group: 'Edge' },
    ].concat(settingsFields(e.settings || {}, d.keys || {}).map((f) => Object.assign(f, { group: 'Settings' })));
    return K().configure({ title: `${e.type} edge${e.tag ? ' — ' + e.tag : ''}`, sub: `${nodeName(e.from)} → ${nodeName(e.to)} · ${d.desc || ''}`, readOnly: ro, allowAdd: !ro,
      meta: [['type', esc(e.type)], ['state', e.active === false ? 'inactive' : 'active'], ['id', `<span class="g-mono">${esc(e.id)}</span>`]],
      fields,
      onSave: async ({ changed, removed }) => {
        const body = {}; const settings = {};
        Object.keys(changed).forEach((k) => {
          if (k === '__tag') body.tag = changed[k]; else if (k === '__enabled') body.active = changed[k]; else if (k === '__from') body.from = changed[k]; else if (k === '__to') body.to = changed[k]; else settings[k] = changed[k];
        });
        removed.forEach((k) => { if (!k.startsWith('__')) settings[k] = null; });
        if (Object.keys(settings).length) body.settings = settings;
        if (!Object.keys(body).length) return true;
        try { await api('/api/settings/edges/' + enc(e.id), { method: 'PATCH', body: JSON.stringify(body) }); } catch (er) { throw new Error(errMsg(er)); }
        toast('Edge saved', { kind: 'ok' }); reload({ kind: 'edge', id: e.id }); if (S.ctx.onChange) S.ctx.onChange(); return true;
      } });
  }
  async function patchEdge(e, body, msg) {
    try { await api('/api/settings/edges/' + enc(e.id), { method: 'PATCH', body: JSON.stringify(body) }); }
    catch (er) { toast('Refused: ' + errMsg(er), { kind: 'err', ms: 6000 }); return; }
    toast(msg || 'Saved', { kind: 'ok' }); await reload({ kind: 'edge', id: e.id }); if (S.ctx.onChange) S.ctx.onChange();
  }
  async function tagEdge(e) {
    const tag = await K().ask({ title: 'Tag this edge', label: 'Tag', value: e.tag || '', placeholder: 'e.g. pdy_default', help: 'Shown on the edge in place of its type; blank removes the tag.', okLabel: 'Save tag' });
    if (tag == null) return;
    patchEdge(e, { tag }, tag ? `Tagged ${tag}` : 'Tag removed');
  }
  function toggleEdge(e) { patchEdge(e, { active: e.enabled === false }, e.enabled === false ? 'Edge switched on' : 'Edge switched off — its settings are kept'); }
  async function deleteEdge(e) {
    const ok = await K().confirm({ title: `Delete this ${e.type} edge?`, text: `<b>${esc(nodeName(e.from))}</b> → <b>${esc(nodeName(e.to))}</b>${e.tag ? ` (${esc(e.tag)})` : ''}. Its settings go with it. To pause it instead, switch it off or drag an end onto the deactivated placeholder.`, okLabel: 'Delete edge', danger: true });
    if (!ok) return;
    try { await api('/api/settings/edges/' + enc(e.id), { method: 'DELETE' }); } catch (er) { toast('Refused: ' + errMsg(er), { kind: 'err', ms: 6000 }); return; }
    toast('Edge deleted', { kind: 'ok' }); select(null); reload(); if (S.ctx.onChange) S.ctx.onChange();
  }
  async function deleteNode(n) {
    if (!n || n.placeholder || n.type === 'iter_data') return;
    const ne = S.data.edges.filter((e) => e.from === n.id || e.to === n.id).length;
    const ok = await K().confirm({ title: `Delete ${nodeName(n.id)}?`, text: `The ${esc((TYPES[n.type] || {}).label || n.type)} and its settings go.${ne ? ` Its ${ne} edge(s) move to the deactivated placeholder (kept, switched off).` : ''}`, okLabel: 'Delete', danger: true });
    if (!ok) return;
    try { await api('/api/settings/nodes/' + enc(n.id), { method: 'DELETE' }); } catch (er) { toast('Refused: ' + errMsg(er), { kind: 'err', ms: 6000 }); return; }
    toast('Deleted', { kind: 'ok' }); select(null); reload(); if (S.ctx.onChange) S.ctx.onChange();
  }
  async function newNode(type) {
    if (!admin()) return;
    const types = TYPE_ORDER.filter((t) => t !== 'iter_data');
    let created = null;
    await K().modal({ title: 'New settings node', wide: false,
      body: `<label class="kit-field"><span>Type</span><select id="sn-type">${types.map((t) => `<option value="${t}" ${t === (type || 'iter_engine') ? 'selected' : ''}>${esc(TYPES[t].long)}</option>`).join('')}</select></label>
        <label class="kit-field"><span>Name</span><input id="sn-name" type="text" autocomplete="off" spellcheck="false" placeholder="letters, digits, - _ ." autofocus><small id="sn-help"></small></label>
        <p class="kit-text kit-dim">Settings are added afterwards with Configure. Engines also register themselves the first time they start (<code>iter_engine --data-url URL --env-file PATH</code>).</p>`,
      buttons: [{ label: 'Cancel', value: null }, { label: 'Create', value: 'ok', primary: true }],
      onOpen: (d) => { const t = d.querySelector('#sn-type'); const h = d.querySelector('#sn-help'); const sync = () => { h.textContent = { project: 'A designed project: build it later from its Project graph. (The wizard on the Intro tab does the same.)', account: 'Connect it to a provider (of), to projects (bills) and to the engines holding its token (holds).', user: 'Set a password from the user gear in the work queue.' }[t.value] || ''; }; t.onchange = sync; sync(); },
      onButton: async (v, d) => {
        const t = d.querySelector('#sn-type').value; const name = d.querySelector('#sn-name').value.trim();
        if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$/.test(name)) throw new Error('A name of letters, digits, dot, dash or underscore.');
        const r = await api('/api/settings/nodes', { method: 'POST', body: JSON.stringify({ type: t, name, settings: {} }) }).catch((e) => { throw new Error(errMsg(e)); });
        created = (r && r.id) || `${t}:${name}`; S.hide.delete(t);
        return true;
      } });
    if (created) { toast('Created ' + created, { kind: 'ok' }); await reload(); focus(created); if (S.ctx.onChange) S.ctx.onChange(); }
  }
  function copyEdge(e) { K().clip.settings = { id: e.id, type: e.type, from: e.from, to: e.to, tag: e.tag }; toast(`Copied the ${e.type} edge${e.tag ? ' ' + e.tag : ''} and its settings. Select a node and press ${K().MOD}V.`, { ms: 4500 }); renderBar(); if (S.sel) renderDetail(); }
  async function pasteEdge(n) {
    const c = K().clip.settings; if (!c || !admin() || !n) return;
    const opts = [];
    if (edgeDef(c.type, n.type, typeOfId(c.to)) && n.id !== c.from) opts.push({ value: { from: n.id }, label: `${nodeName(n.id)} → ${nodeName(c.to)}`, desc: `replaces the start (${nodeName(c.from)})` });
    if (edgeDef(c.type, typeOfId(c.from), n.type) && n.id !== c.to) opts.push({ value: { to: n.id }, label: `${nodeName(c.from)} → ${nodeName(n.id)}`, desc: `replaces the end (${nodeName(c.to)})` });
    if (!opts.length) { toast(`A ${c.type} edge cannot attach to a ${n.type}.`, { kind: 'err' }); return; }
    const pick = opts.length === 1 ? opts[0].value : await K().choose({ title: `Paste ${c.type} edge`, text: `Which end does <b>${esc(nodeName(n.id))}</b> take? The copy keeps the tag and settings.`, options: opts, okLabel: 'Paste' });
    if (!pick) return;
    let r;
    try { r = await api('/api/settings/edges/' + enc(c.id) + '/copy', { method: 'POST', body: JSON.stringify(pick) }); } catch (er) { toast('Refused: ' + errMsg(er), { kind: 'err', ms: 6000 }); return; }
    toast('Edge pasted', { kind: 'ok' }); await reload(r && r.id ? { kind: 'edge', id: r.id } : null); if (S.ctx.onChange) S.ctx.onChange();
  }
  function startDraw(n) {
    if (!admin() || !n || n.placeholder) return;
    cancelDraw();
    const cy = S.cy; const src = cy.getElementById(n.id); if (src.empty()) return;
    const p = src.position();
    cy.add([{ group: 'nodes', data: Object.assign({}, K().HELPER_NODE, { id: '__sdraw_c' }), position: { x: p.x + 40, y: p.y + 40 }, selectable: false, grabbable: false, classes: 'g-draw-cursor' },
      { group: 'edges', data: Object.assign({}, K().HELPER_EDGE, { id: '__sdraw_e', source: n.id, target: '__sdraw_c' }), selectable: false, classes: 'g-draw-edge' }]);
    cy.style().selector('.g-draw-cursor').style({ width: 6, height: 6, 'background-color': '#4da3ff', label: '', events: 'no' })
      .selector('.g-draw-edge').style({ 'line-color': '#4da3ff', 'line-style': 'dashed', width: 2, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#4da3ff', 'curve-style': 'straight', opacity: 1, events: 'no', label: '' }).update();
    const move = (e) => { const c = cy.getElementById('__sdraw_c'); if (c.nonempty()) c.position(e.position); };
    cy.on('mousemove', move);
    S.draw = { from: n, move };
    const targets = EDGE_TYPES.filter((d) => d.from === n.type).map((d) => `${d.edge} → ${d.to}`).concat(EDGE_TYPES.filter((d) => d.to === n.type).map((d) => `${d.from} ← ${d.edge}`));
    banner(`Connecting from <b>${esc(nodeName(n.id))}</b>: click a ${esc(targets.join(', ') || 'node')}. <kbd>Esc</kbd> cancels.`);
  }
  function cancelDraw() {
    if (S.draw && S.cy) { S.cy.off('mousemove', S.draw.move); S.cy.remove('#__sdraw_c, #__sdraw_e'); banner(''); }
    S.draw = null;
  }
  async function finishDraw(id) {
    const a = S.draw.from; cancelDraw();
    const b = S.byId.get(id); if (!b || b.id === a.id) return;
    let from = a; let to = b; let type = typeFor(a.type, b.type);
    if (!type && typeFor(b.type, a.type)) { from = b; to = a; type = typeFor(b.type, a.type); }
    if (!type) { toast(`No edge type joins a ${a.type} and a ${b.type}.`, { kind: 'err', ms: 5000 }); return; }
    const d = edgeDef(type, from.type, to.type);
    const tag = await K().ask({ title: `New ${type} edge`, text: `<b>${esc(nodeName(from.id))}</b> → <b>${esc(nodeName(to.id))}</b>: ${esc(d.desc)}.${Object.keys(d.keys).length ? ` Settings (${Object.keys(d.keys).join(', ')}) follow in Configure.` : ''}`, label: 'Tag (optional)', placeholder: 'e.g. pdy_default', okLabel: 'Add edge' });
    if (tag == null) return;
    let r;
    try { r = await api('/api/settings/edges', { method: 'POST', body: JSON.stringify(Object.assign({ type, from: from.id, to: to.id }, tag ? { tag } : {})) }); }
    catch (er) { toast('Refused: ' + errMsg(er), { kind: 'err', ms: 6000 }); return; }
    toast(`${type} edge added`, { kind: 'ok' });
    await reload(r && r.id ? { kind: 'edge', id: r.id } : null);
    if (S.ctx.onChange) S.ctx.onChange();
    if (r && r.id && Object.keys(d.keys).length) { const e = S.data.edges.find((x) => x.id === r.id); if (e) configureEdge(e); }
  }
  function banner(h) { const b = $('banner'); if (!b) return; b.innerHTML = h; b.classList.toggle('hidden', !h); }

  // ------------------------------------------------------------------ search + keys + help
  function applySearch() {
    const q = ($('search').value || '').trim().toLowerCase(); const cy = S.cy; if (!cy) return [];
    cy.elements().removeClass('match');
    if (q.length < 2) return [];
    const hits = cy.elements().filter((el) => !isHelper(el) && String(el.data('label') || '').toLowerCase().includes(q));
    hits.addClass('match');
    return hits;
  }
  function openHelp() {
    K().help('Settings graph — shortcuts', [
      ['Moving around', [['/', 'Find a node or tag'], ['F', 'Fit'], ['R', 'Run the layout again'], ['Esc', 'Clear the selection'], ['?', 'This help']]],
      ['Editing (admin)', [['N', 'New node'], ['E / double-click', 'Configure the selection'], ['C', 'Connect from the selected node'], ['Drag an edge end', 'Move it to another node (onto a placeholder: switch off)'], ['Mod+C / Mod+V', 'Copy an edge, paste it (with its settings) onto the selected node'], ['Del', 'Delete the selection']]],
    ], 'An edge type follows from its two ends: an engine → project edge is serves, account → project is bills, and so on.');
  }
  function onKey(ev) {
    if (!S.el || !S.el.offsetParent || !S.cy || K().dialogOpen()) return;
    if (ev.key === 'Escape' && S.draw) { ev.preventDefault(); cancelDraw(); return; }
    const typing = K().typing();
    if (ev.key === '/' && !typing && !ev.metaKey && !ev.ctrlKey) { ev.preventDefault(); $('search').focus(); $('search').select(); return; }
    if (typing) return;
    const mod = ev.metaKey || ev.ctrlKey;
    const n = S.sel && S.sel.kind === 'node' ? S.byId.get(S.sel.id) : null;
    const e = S.sel && S.sel.kind === 'edge' ? S.data.edges.find((x) => x.id === S.sel.id) : null;
    if (mod && !ev.altKey && (ev.key === 'c' || ev.key === 'C')) { if (e) { ev.preventDefault(); copyEdge(e); } return; }
    if (mod && !ev.altKey && (ev.key === 'v' || ev.key === 'V')) { if (n && K().clip.settings) { ev.preventDefault(); pasteEdge(n); } return; }
    if (mod || ev.altKey) return;
    const k = ev.key.toLowerCase();
    if (ev.key === 'Escape' && S.sel) select(null);
    else if (k === 'f') { ev.preventDefault(); S.cy.animate({ fit: { eles: S.cy.elements().not('.kit-ep'), padding: 30 } }, { duration: 250 }); }
    else if (k === 'r') { ev.preventDefault(); layout(); }
    else if (ev.key === '?') { ev.preventDefault(); openHelp(); }
    else if (k === 'e' || ev.key === 'Enter') { if (n) { ev.preventDefault(); configureNode(n); } else if (e) { ev.preventDefault(); configureEdge(e); } }
    else if (!admin()) return;
    else if (k === 'n') { ev.preventDefault(); newNode(); }
    else if (k === 'c' && n) { ev.preventDefault(); startDraw(n); }
    else if ((ev.key === 'Delete' || ev.key === 'Backspace') && (n || e)) { ev.preventDefault(); if (n) deleteNode(n); else deleteEdge(e); }
  }

  function mount(el, ctx) {
    S.el = el; S.ctx = ctx || {};
    try { const h = JSON.parse(localStorage.getItem('iter5.settings.hide') || 'null'); if (Array.isArray(h)) S.hide = new Set(h); else S.hide = new Set(['workitem_type', 'agent_tools']); } catch (e) { S.hide = new Set(['workitem_type', 'agent_tools']); }
    try { const l = localStorage.getItem('iter5.settings.layout'); if (['columns', 'flow', 'force'].includes(l)) S.layout = l; } catch (e) { /* storage blocked */ }
    el.innerHTML = TEMPLATE;
    el.classList.add('gview');
    renderLegend();
    const saveHide = () => { try { localStorage.setItem('iter5.settings.hide', JSON.stringify([...S.hide])); } catch (e) { /* storage blocked */ } };
    $('chips').addEventListener('click', (ev) => { const b = ev.target.closest('[data-type]'); if (!b || !S.data) return; const k = b.dataset.type;
      if (ev.altKey) S.hide = new Set(TYPE_ORDER.filter((x) => x !== k)); else if (S.hide.has(k)) S.hide.delete(k); else S.hide.add(k);
      saveHide(); draw(S.layout === 'columns' ? false : true); });
    el.querySelector('#s-layouts').addEventListener('click', (ev) => { const b = ev.target.closest('[data-layout]'); if (!b) return; S.layout = b.dataset.layout; try { localStorage.setItem('iter5.settings.layout', S.layout); } catch (e) { /* storage blocked */ } renderChips(); layout(); });
    $('ph').onchange = () => draw(false); $('inactive').onchange = () => draw(true);
    $('fit').onclick = () => S.cy && S.cy.animate({ fit: { eles: S.cy.elements().not('.kit-ep'), padding: 30 } }, { duration: 250 });
    $('relayout').onclick = () => S.cy && layout();
    $('help').onclick = openHelp;
    $('search').addEventListener('input', applySearch);
    $('search').addEventListener('keydown', (ev) => { if (ev.key === 'Enter') { const h = applySearch(); if (h && h.length) { const el1 = h[0]; if (el1.isNode()) focus(el1.id()); else select({ kind: 'edge', id: el1.data('eid') }); } } if (ev.key === 'Escape') { $('search').value = ''; applySearch(); $('search').blur(); } });
    $('detail').addEventListener('click', (ev) => { const idEl = ev.target.closest('.g-id'); if (idEl && navigator.clipboard) navigator.clipboard.writeText(idEl.textContent).then(() => toast('Copied', { kind: 'ok', ms: 1200 })).catch(() => {}); });
    if (!S.keyBound) { document.addEventListener('keydown', onKey); S.keyBound = true; }
    // refresh in the background while visible (engines and other admins change things)
    S.timer = setInterval(() => { if (S.el.offsetParent && !K().dialogOpen() && !S.draw && S.data) load(true).then(() => { if (S.sel) { const ok = S.sel.kind === 'node' ? S.byId.has(S.sel.id) : S.data.edges.some((e) => e.id === S.sel.id); if (ok) renderDetail(); else select(null); } }); }, 20000);
    return show(ctx);
  }
  async function show(ctx) {
    if (ctx) S.ctx = ctx;
    const first = !S.data;
    await load(!first);
    if (S.cy) S.cy.resize();
    if (first && S.cy) layout();
    if (S.pending) { const p = S.pending; S.pending = null; focus(p); }
  }
  window.IterSettings = { mount, show, focus: (id) => focus(id), resize: () => { if (S.cy) { S.cy.resize(); } }, newNode: (t) => newNode(t), get mounted() { return !!S.el; }, get cy() { return S.cy; } };
}());
