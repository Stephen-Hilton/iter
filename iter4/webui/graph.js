/*
 * iter4 Project graph — the architecture map of one project, drawn from
 * GET /api/projects/{p}/graph/view. Ported (2026-09-28) from pdy-dev's
 * demos/usecase_map viewer (app.js), which reads the same shape; changes:
 * mounted as a tab of the iter4 webui (element ids prefixed "g-", one set of
 * listeners per load), dark theme, the edge toggles unlocked, and the URL
 * hash shared with the app (the "tab" and "p" keys are kept).
 *
 * From the original:
 *   - one table of node colours per level and edge styles, read by both the stylesheet and the legend;
 *   - parallel interface edges between the same two nodes are drawn as ONE edge (the detail panel
 *     lists every interface behind it); use-case steps are NOT merged: every step between the same
 *     two parts gets its own curve and label (spreadParallel), so each one can be read and clicked;
 *   - "tiered", "rings" and "sequence" are pure position maths applied as a preset layout; "cluster"
 *     is fcose over compound nodes; "flow" is dagre, left to right. Every layout is only a starting
 *     arrangement - nodes stay draggable.
 */
(function () {
  'use strict';
  let current = null; // {cy, abort} of the drawing on screen
  const TEMPLATE = `
<div id="g-bar" class="g-bar">
    <div class="g-summaryrow"><span id="g-summary"></span><span class="g-edit" id="g-editbar"></span></div>
  <div class="controls">
    <div class="group">
      <span class="lbl">View</span>
      <select id="g-ucpick" aria-label="Use case"></select>
      <label class="chk ucopt" title="Keep the rest of the repository on screen, faded, instead of hiding it">
        <input type="checkbox" id="g-dim"> dim the rest</label>
    </div>
    <div class="group">
      <span class="lbl">Layout</span>
      <div class="seg" id="g-layouts">
        <button data-layout="cluster" title="Boxes inside boxes: each context holds its containers, each container its components (force-directed)">Cluster</button>
        <button data-layout="tiered" title="Rows from top to bottom: actors, contexts, containers, components - or, for a use case, the order the journey reaches each part">Top-down tiered</button>
        <button data-layout="rings" title="Rings from the centre outward: project and contexts in the middle, actors outermost - or, for a use case, the use case at the centre and parts in the order they are reached">Inside-out rings</button>
        <button data-layout="sequence" title="One use case only: one row per step, top to bottom">Sequence</button>
        <button data-layout="flow" title="Left-to-right layered drawing that follows edge direction (dagre)">Flow (left to right)</button>
      </div>
    </div>
    <div class="group ucopt">
      <span class="lbl">Flow</span>
      <div class="seg" id="g-flowkind">
        <button data-flow="process" title="Who calls or triggers whom, in execution order (the default). Steps are numbered 1, 2, 3 ...">Process</button>
        <button data-flow="data" title="Where the information travels and where it comes to rest. Steps are numbered D1, D2, D3 ...">Data</button>
        <button data-flow="both" title="Both flows at once: process steps 1, 2, 3 ... in blue, data steps D1, D2, D3 ... in orange">Both</button>
      </div>
      <select id="g-branch" class="hidden" aria-label="Branch"></select>
    </div>
    <div class="group">
      <span class="lbl">Edges</span>
      <label class="chk" title="Calls between services, events and shared datasets recorded in the interface files"><input type="checkbox" id="g-showIface"> interfaces</label>
      <label class="chk" title="Build-time library links (about 750 of them) - off by default because they swamp the picture"><input type="checkbox" id="g-showLib"> libraries</label>
      <label class="chk" title="Parent to child links (context to container, container to component)"><input type="checkbox" id="g-showContains"> contains</label>
      <label class="chk" title="Links that the code shows are not built yet or not working yet - drawn red and dashed"><input type="checkbox" id="g-showBroken"> not built yet</label>
      <label class="chk wholeopt" title="Add the use cases as nodes, linked to every part they touch"><input type="checkbox" id="g-showUsecases"> use-case nodes</label>
    </div>
    <div class="group">
      <div class="searchbox">
        <input type="search" id="g-search" placeholder="Search nodes and steps (⌘F)" aria-label="Search nodes and steps" autocomplete="off" spellcheck="false">
        <div id="g-searchlist" class="hidden"></div>
      </div>
      <button class="plain" id="g-fit" title="Fit the whole drawing on screen">Fit</button>
      <button class="plain" id="g-relayout" title="Run the current layout again">Re-layout</button>
      <button class="plain" id="g-showhidden" title="Put back every node you hid with Hide this node" disabled>Show all hidden nodes</button>
    </div>
  </div>
</div>
<div class="g-main">
  <div id="g-cywrap">
    <div id="g-cy"></div>
    <div id="g-empty" class="hidden"></div>
    <div id="g-ucsummary" class="hidden"></div>
    <div id="g-banner" class="hidden"></div>
    <div id="g-tip" class="hidden"></div>
  </div>
  <aside id="g-detail" class="g-detail hidden"></aside>
</div>
<div id="g-legend" class="g-legend"></div>
`;
  let registered = false;
  window.IterGraph = {
    /** Draw graph G (the /graph/view JSON) into root; any earlier drawing is torn down. */
    draw(root, G, ctx) {
      if (current) {
        try { current.abort.abort(); } catch (e) { /* old browser */ }
        try { current.cy && current.cy.destroy(); } catch (e) { /* already gone */ }
        current = null;
      }
      root.innerHTML = TEMPLATE;
      if (!registered) {
        if (typeof cytoscapeFcose !== 'undefined') cytoscape.use(cytoscapeFcose);
        if (typeof cytoscapeDagre !== 'undefined') cytoscape.use(cytoscapeDagre);
        registered = true;
      }
      const abort = new AbortController();
      current = { abort, cy: null };
      current.cy = start(root, G, ctx || {}, abort.signal);
      return current.cy;
    },
    /** The live drawing's handles (scripted checks). */
    get current() { return current; },
  };

  function start(ROOT, G, CTX, SIGNAL) {
  const $ = (id) => document.getElementById('g-' + id);
  const onDoc = (type, fn) => document.addEventListener(type, fn, { signal: SIGNAL });

  // ------------------------------------------------------------------ visual encoding (one place)
  const LEVELS = {
    project: { label: 'Project (the whole repository)', color: '#334155', border: '#0f172a', shape: 'octagon', w: 44, h: 44, font: 15 },
    context: { label: 'Context (a major area of the system)', color: '#7c3aed', border: '#4c1d95', shape: 'round-rectangle', w: 44, h: 30, font: 14 },
    container: { label: 'Container (one deployable program)', color: '#2563eb', border: '#1e3a8a', shape: 'round-rectangle', w: 30, h: 22, font: 12 },
    component: { label: 'Component (a part inside a program)', color: '#0d9488', border: '#134e4a', shape: 'round-rectangle', w: 22, h: 16, font: 11 },
    actor: { label: 'Actor (a person or outside organisation)', color: '#d97706', border: '#78350f', shape: 'ellipse', w: 34, h: 34, font: 13 },
    usecase: { label: 'Use case (one journey)', color: '#db2777', border: '#831843', shape: 'round-diamond', w: 44, h: 44, font: 14 },
  };
  const LEVEL_ORDER = ['project', 'actor', 'context', 'container', 'component', 'usecase'];
  const KIND_COLOR = { 'request-reply': '#cbd5e1', event: '#9333ea', dataset: '#0891b2' };
  const KIND_TEXT = { 'request-reply': 'call and answer', event: 'event (published message)', dataset: 'shared data read by another part' };
  const BAD = '#dc2626';
  const EDGE_LEGEND = [
    { label: 'Interface: call and answer', color: KIND_COLOR['request-reply'], line: 'solid' },
    { label: 'Interface: event', color: KIND_COLOR.event, line: 'solid' },
    { label: 'Interface: shared dataset', color: KIND_COLOR.dataset, line: 'solid' },
    { label: 'Library link (build time)', color: '#a8b3c4', line: 'dotted' },
    { label: 'Contains (parent to child)', color: '#a3adbd', line: 'dotted', noArrow: true },
    { label: 'Use case: process step (1, 2, 3 ...)', color: '#1d4ed8', line: 'solid', thick: true },
    { label: 'Use case: data step (D1, D2, D3 ...; * = on a branch, as in 5* or D5*)', color: '#ea580c', line: 'solid', thick: true },
    { label: 'Not built or not working yet', color: BAD, line: 'dashed' },
    { label: 'Use case starts here / touches', color: '#f472b6', line: 'solid' },
  ];
  /** Statuses that mean the link or step works: 'live' (interface edges) and 'complete' (a flowmap
   *  step or edge marked finished). No status at all also counts as good. Anything else - not-built,
   *  broken, not-deployed or an unknown word - is drawn red and dashed. */
  const GOOD_STATUS = new Set(['live', 'complete']);
  const isBad = (status) => !!status && !GOOD_STATUS.has(String(status).toLowerCase());
  /** What a step is called everywhere on the page: process steps are plain numbers, data steps carry a D. */
  const stepName = (e) => (e.kind === 'data' ? 'D' : '') + (e.step == null ? '?' : String(e.step));

  // ------------------------------------------------------------------ indexes
  const nodesById = new Map(G.nodes.map((n) => [n.id, n]));
  const ucById = new Map(G.usecases.map((u) => [u.id, u]));
  const ifaceEdges = G.edges.filter((e) => e.type === 'interface');
  const containsEdges = G.edges.filter((e) => e.type === 'contains');
  const touchEdges = G.edges.filter((e) => e.type === 'usecase_touches');
  const flowsByUc = new Map();
  for (const e of G.edges) {
    if (e.type !== 'flow') continue;
    if (!flowsByUc.has(e.usecase)) flowsByUc.set(e.usecase, []);
    flowsByUc.get(e.usecase).push(e);
  }
  const nameOf = (id) => (nodesById.get(id) || {}).name || id;
  /** What an interface does, in words (its file's `label:`), else its id with the dashes read as spaces. */
  const ifLabel = (e) => (e && (e.label || String(e.interface || '').replace(/[-_]+/g, ' '))) || '';

  // ------------------------------------------------------------------ state (mirrored in the URL hash)
  const DEFAULTS = {
    uc: '', layout: 'cluster', flow: 'process', branch: '', dim: false,
    showIface: true, showLib: false, showContains: false, showBroken: true, showUsecases: false,
  };
  const state = Object.assign({}, DEFAULTS);
  // Demo mode (Stephen, 2026-09-28): the four Edges toggles (interfaces, libraries, contains,
  // not built / broken) are hidden in index.html and held off here, whatever the URL says.
  // Set EDGE_TOGGLES_LOCKED to false and remove the `demo-hidden` class there to bring them back.
  const EDGE_TOGGLES_LOCKED = false; // iter4: the edge toggles are part of the tab
  const LOCKED_EDGE_KEYS = ['showIface', 'showLib', 'showContains', 'showBroken'];
  function lockEdges() { if (EDGE_TOGGLES_LOCKED) for (const k of LOCKED_EDGE_KEYS) state[k] = false; }
  const BOOLS = ['dim', 'showIface', 'showLib', 'showContains', 'showBroken', 'showUsecases'];
  function readHash() {
    const p = new URLSearchParams(location.hash.slice(1));
    Object.assign(state, DEFAULTS);
    for (const k of Object.keys(DEFAULTS)) {
      if (!p.has(k)) continue;
      state[k] = BOOLS.includes(k) ? p.get(k) === '1' : p.get(k);
    }
    if (state.uc && !ucById.has(state.uc)) state.uc = '';
    if (!['cluster', 'tiered', 'rings', 'sequence', 'flow'].includes(state.layout)) state.layout = 'cluster';
    if (!['process', 'data', 'both'].includes(state.flow)) state.flow = DEFAULTS.flow;    lockEdges();
  }
  function writeHash() {
    // the app's own keys (tab, p = project) ride along; the viewer owns the rest
    const keep = new URLSearchParams(location.hash.slice(1));
    const p = new URLSearchParams();
    for (const k of ['tab', 'p']) if (keep.has(k)) p.set(k, keep.get(k));
    for (const k of Object.keys(DEFAULTS)) {
      if (state[k] === DEFAULTS[k]) continue;
      p.set(k, BOOLS.includes(k) ? (state[k] ? '1' : '0') : state[k]);
    }
    history.replaceState(null, '', '#' + p.toString());
  }

  // ------------------------------------------------------------------ what is on screen
  /**
   * Whole repository in a row or ring layout: the contains links are always drawn. Cluster shows
   * containment as boxes inside boxes; tiered and rings place a context in its own row or ring, and
   * without these links a context (which calls nothing itself - its containers do) sits there joined
   * to nothing. Flow and Sequence fall back to tiered without a use case, so they count too.
   */
  const containsForced = () => !state.uc && state.layout !== 'cluster';
  /** Nodes the presenter hid with "Hide this node" (Stephen, 2026-09-28): left out of every view with
   *  all their edges until "Show all hidden nodes". Kept for this browser tab only (sessionStorage),
   *  so a reload mid-talk keeps them hidden and a new tab starts with everything shown. */
  const HIDDEN = new Set();
  try { JSON.parse(sessionStorage.getItem('ucmap.hidden') || '[]').forEach((id) => HIDDEN.add(id)); } catch (e) { /* storage blocked */ }
  function saveHidden() {
    try { sessionStorage.setItem('ucmap.hidden', JSON.stringify([...HIDDEN])); } catch (e) { /* storage blocked */ }
  }
  function hideNode(id) { HIDDEN.add(id); saveHidden(); select(null); render(); }
  function showAllHidden() { HIDDEN.clear(); saveHidden(); render(); }
  function currentView() {
    const uc = state.uc ? ucById.get(state.uc) : null;
    const touched = new Set();
    let flowEdges = [];
    if (uc) {
      touched.add(uc.id);
      // Parts on a step hidden by the Process/Data or branch filter leave the view; parts the
      // sequence lists but no step mentions stay (they are part of the journey as written).
      const onAnyStep = new Set();
      (flowsByUc.get(uc.id) || []).forEach((e) => { onAnyStep.add(e.source); onAnyStep.add(e.target); });
      uc.sequence.forEach((id) => { if (!onAnyStep.has(id)) touched.add(id); });
      flowEdges = (flowsByUc.get(uc.id) || []).filter((e) =>
        (state.flow === 'both' || e.kind === state.flow)
        && (!state.branch || !e.branch || e.branch === state.branch)
        && (state.showBroken || !isBad(e.status))
        && !HIDDEN.has(e.source) && !HIDDEN.has(e.target));
      flowEdges.forEach((e) => { touched.add(e.source); touched.add(e.target); });
    }
    const whole = !uc || state.dim;
    const ids = whole
      ? G.nodes.filter((n) => n.level !== 'usecase' || (!uc && state.showUsecases) || (uc && n.id === uc.id)).map((n) => n.id)
      : [...touched].filter((id) => nodesById.has(id));
    const shown = ids.filter((id) => !HIDDEN.has(id));
    ids.length = 0; ids.push(...shown);
    const idset = new Set(ids);
    const edges = [];
    const both = (e) => idset.has(e.source) && idset.has(e.target);
    if (state.showIface) {
      for (const e of ifaceEdges) {
        if (both(e) && (state.showLib || !e.is_library) && (state.showBroken || !isBad(e.status))) edges.push(e);
      }
    }
    if (state.showContains || containsForced()) containsEdges.forEach((e) => { if (both(e)) edges.push(e); });
    if (!uc && state.showUsecases) touchEdges.forEach((e) => { if (both(e)) edges.push(e); });
    if (uc) {
      edges.push(...flowEdges);
      if (uc.has_flowmap) {
        const first = [...flowEdges].sort((a, b) => (a.step ?? 1e9) - (b.step ?? 1e9) || (a.kind === 'process' ? -1 : 1))[0];
        const start = first ? first.source : uc.sequence[0];
        if (start && idset.has(start)) {
          edges.push({ id: 'start|' + uc.id, type: 'start', source: uc.id, target: start });
        }
      } else {
        // iter4's touch edges reach only a use case's top-level parts: link every part it lists
        uc.sequence.forEach((id, i) => {
          if (idset.has(id)) edges.push({ id: `touch|${uc.id}|${id}`, type: 'usecase_touches', source: uc.id, target: id, usecase: uc.id, order: i + 1 });
        });
      }
    }
    return { uc, ids, idset, edges, touched, flowEdges, whole };
  }

  /** Parallel interface edges between the same two nodes become one drawn edge. */
  function mergeEdges(list) {
    const out = new Map();
    for (const e of list) {
      const key = e.type === 'interface' ? `if|${e.source}|${e.target}|${e.is_library ? 'lib' : 'rt'}` : e.id;
      let m = out.get(key);
      if (!m) { m = { key, type: e.type, source: e.source, target: e.target, members: [] }; out.set(key, m); }
      m.members.push(e);
    }
    return [...out.values()];
  }

  /**
   * Every use-case step is its own edge, and a journey often makes several steps between the same
   * two parts (worker to gateway at steps 1, 6 and 11; a call and its data write in both flows).
   * Given one shared curve (the tiered layout's bow used to be the same for every step of a pair)
   * they sit exactly on top of each other and only the last label shows; Cytoscape's own bundling
   * of plain edges only puts labels about 20px apart. So edges between the same two parts, in either
   * direction, get their own curves: steps that would be drawn straight fan out on both sides of the
   * straight line, steps that share a bow fan outward from it. Only pairs that carry a use-case step
   * are spread here; other pairs keep Cytoscape's own bundling.
   * A curve's midpoint (where its label sits) moves half its control-point distance, so the gap is
   * twice the widest label in the pair plus a margin: labels "1" and "12*" need different room.
   * Returns, per edge, the control-point distance (relative to that edge's own direction, as
   * Cytoscape reads it), or null for "draw it the ordinary way".
   */
  const LABEL_CHAR_PX = 10; const LABEL_PAD_PX = 14; const MIN_GAP = 40;
  function spreadParallel(list, bow) {
    const out = list.map((m, i) => (bow[i] ? bow[i] : null));
    const groups = new Map();
    list.forEach((m, i) => {
      if (m.source === m.target) return; // a loop on one part: Cytoscape already stacks loops
      const key = m.source < m.target ? m.source + '\u0000' + m.target : m.target + '\u0000' + m.source;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(i);
    });
    const rank = (m) => {
      const e = m.members[0];
      const k = m.type === 'flow' ? (e.kind === 'process' ? 0 : 1) : m.type === 'start' ? -1 : 2;
      return [k, e.step ?? 1e9, e.branch || ''];
    };
    groups.forEach((idx) => {
      if (idx.length < 2 || !idx.some((i) => list[i].type === 'flow')) return;
      // Cytoscape measures the distance to the left of the edge's own direction, so an edge that runs
      // the other way round the pair has its sign flipped: work in the pair's canonical direction.
      const dir = (i) => (list[i].source < list[i].target ? 1 : -1);
      const base = (i) => (bow[i] || 0) * dir(i);
      const widest = Math.max(...idx.map((i) => edgeVisual(list[i]).label.length));
      const gap = Math.max(MIN_GAP, 2 * (widest * LABEL_CHAR_PX + LABEL_PAD_PX));
      // Edges sharing one bow form one fan. A fan drawn straight spreads evenly on both sides; a bowed
      // fan spreads outward from its bow, so no step is pulled back in across the nodes the bow avoids
      // (and a return step's big bow does not drag the straight steps of the same pair sideways).
      const fans = new Map();
      idx.forEach((i) => { const k = Math.round(base(i)); if (!fans.has(k)) fans.set(k, []); fans.get(k).push(i); });
      fans.forEach((fan, centre) => {
        const order = [...fan].sort((x, y) => {
          const rx = rank(list[x]); const ry = rank(list[y]);
          return rx[0] - ry[0] || rx[1] - ry[1] || String(rx[2]).localeCompare(String(ry[2]));
        });
        const outward = Math.sign(centre);
        order.forEach((i, j) => {
          const c = outward ? centre + outward * j * gap : centre + (j - (order.length - 1) / 2) * gap;
          out[i] = c * dir(i);
        });
      });
    });
    return out;
  }

  function edgeVisual(m) {
    const e = m.members[0];
    const bad = m.members.some((x) => isBad(x.status));
    let color = '#94a3b8'; let line = 'solid'; let width = 1.3; let arrow = 'triangle'; let label = ''; const cls = [m.type];
    if (m.type === 'interface') {
      if (e.is_library) { color = '#a8b3c4'; line = 'dotted'; width = 1; cls.push('lib'); } else {
        const kinds = {}; m.members.forEach((x) => { kinds[x.kind] = (kinds[x.kind] || 0) + 1; });
        const kind = Object.keys(kinds).sort((a, b) => kinds[b] - kinds[a])[0];
        color = KIND_COLOR[kind] || '#cbd5e1';
        if (m.members.some((x) => x.edge_kind === 'governs')) line = 'dotted';
        width = 1.2 + Math.min(3, Math.log2(m.members.length));
      }
    } else if (m.type === 'contains') { color = '#a3adbd'; line = 'dotted'; width = 1.8; arrow = 'none'; }
    else if (m.type === 'flow') {
      color = e.kind === 'process' ? '#1d4ed8' : '#ea580c'; width = 3;
      // Step number only; the branch name is in the tooltip and the detail panel (long names clutter the drawing).
      label = stepName(e);
      if (e.branch) { label += '*'; cls.push('branch'); }
      cls.push(e.kind);
    } else if (m.type === 'usecase_touches') { color = '#f9a8d4'; width = 1.2; }
    else if (m.type === 'start') { color = '#f472b6'; width = 2.5; label = 'starts'; }
    if (bad) { color = BAD; line = 'dashed'; cls.push('bad'); }
    return { color, line, width, arrow, label, cls: cls.join(' ') };
  }

  // ------------------------------------------------------------------ layout maths (ported from skillsnap graphLayouts.ts)
  function groupBands(ids, bandOf, orderKey) {
    const by = new Map();
    const key = orderKey || (() => 0);
    for (const id of [...ids].sort((a, b) => key(a) - key(b) || (a < b ? -1 : a > b ? 1 : 0))) {
      const b = bandOf(id);
      if (!by.has(b)) by.set(b, []);
      by.get(b).push(id);
    }
    return [...by.keys()].sort((a, b) => a - b).map((k) => by.get(k));
  }
  function adjacency(ids, edges) {
    const adj = new Map(ids.map((id) => [id, new Set()]));
    for (const e of edges) {
      if (e.source === e.target) continue;
      const a = adj.get(e.source); const b = adj.get(e.target);
      if (!a || !b) continue;
      a.add(e.target); b.add(e.source);
    }
    return adj;
  }
  const subRows = (n, perRow) => Math.max(1, Math.ceil(n / perRow));

  function tieredPositions(ids, edges, bandOf, opts) {
    const o = Object.assign({ colGap: 150, subRowGap: 66, tierGap: 170, aspect: 1.7, minPerRow: 6, sweeps: 8 }, opts || {});
    const pos = new Map();
    if (!ids.length) return pos;
    const bands = groupBands(ids, bandOf, o.orderKey);
    const adj = adjacency(ids, edges);
    const bandOfId = new Map();
    bands.forEach((b, i) => b.forEach((id) => bandOfId.set(id, i)));
    // pick the per-row cap whose overall extent is closest to the target aspect ratio
    const sizes = bands.map((b) => b.length);
    const maxBand = Math.max(...sizes);
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
      order.forEach((id, i) => {
        const col = Math.floor(i / k); const row = i % k;
        const stagger = k > 1 ? (row / (k - 1) - 0.5) * 0.5 * o.colGap : 0;
        pos.set(id, { x: (col - (cols - 1) / 2) * o.colGap + stagger, y: top + row * o.subRowGap });
      });
    };
    const orders = bands.map((b) => [...b]);
    orders.forEach((ord, i) => placeBand(ord, tops[i]));
    const reorder = (i, use) => {
      const bary = new Map();
      for (const id of orders[i]) {
        const xs = [...(adj.get(id) || [])].filter((nb) => use(bandOfId.get(nb))).map((nb) => pos.get(nb).x);
        bary.set(id, xs.length ? xs.reduce((a, v) => a + v, 0) / xs.length : pos.get(id).x);
      }
      orders[i].sort((a, b) => bary.get(a) - bary.get(b) || (o.orderKey ? o.orderKey(a) - o.orderKey(b) : 0) || a.localeCompare(b));
      placeBand(orders[i], tops[i]);
    };
    for (let i = 1; i < bands.length; i++) reorder(i, (j) => j < i);
    for (let s = 0; s < o.sweeps; s++) {
      for (let i = bands.length - 2; i >= 0; i--) reorder(i, (j) => j !== i);
      for (let i = 1; i < bands.length; i++) reorder(i, (j) => j !== i);
    }
    return pos;
  }

  const TAU = Math.PI * 2;
  const norm = (a) => ((a % TAU) + TAU) % TAU;
  function circularMean(angles, weights) {
    if (!angles.length) return null;
    let s = 0; let c = 0;
    angles.forEach((a, i) => { const w = weights ? weights[i] : 1; s += w * Math.sin(a); c += w * Math.cos(a); });
    if (Math.abs(s) < 1e-9 && Math.abs(c) < 1e-9) return null;
    return Math.atan2(s, c);
  }
  function ringPositions(ids, edges, bandOf, opts) {
    const o = Object.assign({ minArc: 150, ringGap: 170, subRingGap: 80, maxSubRings: 3, passes: 8 }, opts || {});
    const pos = new Map();
    if (!ids.length) return pos;
    const bands = groupBands(ids, bandOf);
    const adj = adjacency(ids, edges);
    const angle = new Map();
    const rings = [];
    let prevOuter = -Infinity;
    bands.forEach((rids, bi) => {
      const n = rids.length;
      if (bi === 0 && n === 1) { rings.push({ ids: rids, r: 0, k: 1, center: true }); prevOuter = 0; return; }
      const minR = prevOuter === -Infinity ? 0 : prevOuter + o.ringGap;
      const need = (n * o.minArc) / TAU;
      let k = 1;
      if (bi > 0) while (k < o.maxSubRings && need / k > minR + o.subRingGap * k) k++;
      const slots = Math.ceil(n / k) * k;
      const chordR = n > 1 ? o.minArc / (2 * Math.sin(Math.min(Math.PI / 2, (k * Math.PI) / slots))) : 0;
      const r = Math.max(minR, (slots * o.minArc) / TAU / k, chordR);
      rings.push({ ids: rids, r, k, center: false });
      prevOuter = r + (k - 1) * o.subRingGap;
    });
    const setPos = (id, p) => {
      pos.set(id, p);
      if (Math.hypot(p.x, p.y) < 1e-9) angle.delete(id); else angle.set(id, Math.atan2(p.y, p.x));
    };
    const place = (ring, useAll) => {
      const { r, k } = ring; const rids = ring.ids;
      if (ring.center) { setPos(rids[0], { x: 0, y: 0 }); return; }
      const n = rids.length; const target = new Map();
      rids.forEach((id, i) => {
        const nbs = [...(adj.get(id) || [])].filter((nb) => angle.has(nb) && (useAll || !rids.includes(nb)));
        const m = circularMean(nbs.map((nb) => angle.get(nb)));
        target.set(id, norm(m ?? angle.get(id) ?? (i / n) * TAU));
      });
      const order = [...rids].sort((a, b) => target.get(a) - target.get(b) || a.localeCompare(b));
      const step = TAU / (Math.ceil(n / k) * k);
      const offset = circularMean(order.map((id, j) => target.get(id) - j * step)) ?? 0;
      order.forEach((id, j) => {
        const a = norm(offset + j * step); const radius = r + (j % k) * o.subRingGap;
        setPos(id, { x: radius * Math.cos(a), y: radius * Math.sin(a) });
      });
    };
    for (const ring of rings) place(ring, false);
    for (let p = 1; p < o.passes; p++) for (const ring of rings) place(ring, true);
    return pos;
  }

  // Bands: which row / ring each node sits on.
  const WHOLE_TIER = { usecase: -1, actor: 0, project: 1, context: 2, container: 3, component: 4 };
  const WHOLE_RING = { project: 0, context: 1, container: 2, component: 3, actor: 4, usecase: 5 };
  const levelOf = (id) => (nodesById.get(id) || {}).level;

  /**
   * In a use-case view a node's band is how many calls from the journey's starting point it is
   * first reached (hop depth along the primary flow - process, or data when only data is shown):
   * the use case is band 0; every part that starts steps before anything calls it (the actor who
   * begins the journey, a second actor who joins later) is band 1; parts reached in parallel
   * share a row. A part reached only by the other flow sits one row below the part that sends
   * to it. The exact step order is what the Sequence layout draws.
   */
  function ucBands(v) {
    const b = new Map([[v.uc.id, 0]]);
    if (!v.uc.has_flowmap) {
      for (const id of v.ids) if (!b.has(id)) b.set(id, 1 + (WHOLE_TIER[levelOf(id)] ?? 5));
      return { bands: b };
    }
    const primary = state.flow === 'data' ? 'data' : 'process';
    const byStep = (a, c) => (a.step ?? 1e9) - (c.step ?? 1e9);
    let prim = v.flowEdges.filter((e) => e.kind === primary).sort(byStep);
    let other = v.flowEdges.filter((e) => e.kind !== primary).sort(byStep);
    if (!prim.length) { prim = other; other = []; }
    const seen = new Set(); const starts = [];
    const out = new Map();
    for (const e of prim) {
      if (!seen.has(e.source)) starts.push(e.source);
      seen.add(e.source); seen.add(e.target);
      if (!out.has(e.source)) out.set(e.source, []);
      out.get(e.source).push(e.target);
    }
    const queue = [];
    starts.forEach((id) => { if (!b.has(id)) { b.set(id, 1); queue.push(id); } });
    while (queue.length) {
      const id = queue.shift();
      for (const nb of out.get(id) || []) if (!b.has(nb)) { b.set(nb, b.get(id) + 1); queue.push(nb); }
    }
    for (let pass = 0; pass < 5; pass++) {
      for (const e of other) if (b.has(e.source) && !b.has(e.target)) b.set(e.target, b.get(e.source) + 1);
    }
    let max = 0; b.forEach((x) => { max = Math.max(max, x); });
    for (const e of other) { if (!b.has(e.source)) b.set(e.source, 1); if (!b.has(e.target)) b.set(e.target, max + 1); }
    // Parts listed in the sequence but reached by no step shown: order of first encounter, after the rest.
    v.uc.sequence.forEach((id) => { if (!b.has(id)) b.set(id, ++max); });
    for (const id of v.ids) if (!b.has(id)) b.set(id, max + 1);
    return { bands: b };
  }



  /**
   * Sequence layout: a lifeline diagram. One column per part, in the order the steps first reach
   * it, headed by the part itself; one row per step, top to bottom, each drawn as an arrow from
   * the sender's line to the receiver's with the step number on it. With both flows shown, the
   * process steps come first and the data steps follow under their own heading (the two lists
   * number their steps independently, so interleaving them would invent an order).
   */
  const SEQ = { col: 185, row: 40, top: 70 };
  // Helper elements carry every field the stylesheet maps, so Cytoscape has nothing to warn about.
  const BLANK_NODE = { label: '', level: '', color: '#ffffff', border: '#ffffff', shape: 'rectangle', w: 1, h: 1, font: 12 };
  const BLANK_EDGE = { color: '#cbd5e1', line: 'dashed', width: 1.2, arrow: 'none', label: '', cpd: 0 };
  function sequenceElements(v) {
    const els = []; const pos = new Map();
    const byStep = (a, c) => (a.step ?? 1e9) - (c.step ?? 1e9);
    const kinds = state.flow === 'both' ? ['process', 'data'] : [state.flow];
    const rows = [];
    kinds.forEach((k) => {
      const list = v.flowEdges.filter((e) => e.kind === k).sort(byStep);
      if (list.length && kinds.length > 1) rows.push({ section: k });
      list.forEach((e) => rows.push({ e }));
    });
    const parts = []; const seen = new Set();
    rows.forEach((r) => { if (r.e) for (const id of [r.e.source, r.e.target]) if (!seen.has(id)) { seen.add(id); parts.push(id); } });
    const xOf = new Map(parts.map((id, i) => [id, i * SEQ.col]));
    const bottom = SEQ.top + rows.length * SEQ.row;
    for (const id of parts) {
      const n = nodesById.get(id); const st = LEVELS[n.level] || LEVELS.container;
      els.push({ group: 'nodes', classes: `${n.level} focus seqhead`,
        data: { id, label: n.name, level: n.level, color: st.color, border: st.border, shape: st.shape, w: st.w, h: st.h, font: st.font } });
      pos.set(id, { x: xOf.get(id), y: 0 });
      els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: 'll|' + id } });
      pos.set('ll|' + id, { x: xOf.get(id), y: bottom });
      els.push({ group: 'edges', classes: 'lifeline', data: { ...BLANK_EDGE, id: 'lle|' + id, source: id, target: 'll|' + id } });
    }
    rows.forEach((r, i) => {
      const y = SEQ.top + i * SEQ.row;
      if (r.section) {
        const sid = 'sec|' + r.section;
        els.push({ group: 'nodes', classes: 'section', data: { ...BLANK_NODE, id: sid, label: r.section === 'process' ? 'Process flow' : 'Data flow' } });
        pos.set(sid, { x: -SEQ.col * 0.75, y });
        return;
      }
      const e = r.e; const a = `a|${i}|s`; const b = `a|${i}|t`; const self = e.source === e.target;
      els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: a } });
      els.push({ group: 'nodes', classes: 'anchor', data: { ...BLANK_NODE, id: b } });
      pos.set(a, { x: xOf.get(e.source), y });
      pos.set(b, { x: xOf.get(e.target), y: self ? y + SEQ.row * 0.55 : y });
      const m = { key: e.id, type: 'flow', source: e.source, target: e.target, members: [e] };
      const vis = edgeVisual(m); const eid = 's' + i;
      MERGED.set(eid, m);
      els.push({ group: 'edges', classes: `${vis.cls} seqmsg${self ? ' curved' : ''}`,
        data: { id: eid, source: a, target: b, cpd: self ? -45 : 0, color: vis.color, line: vis.line, width: vis.width, arrow: vis.arrow, label: vis.label } });
    });
    return { els, pos, rows: rows.length, parts: parts.length };
  }

  /** Use-case rings: the use case at the centre, then every part along an outward spiral in the order first reached. */
  function spiralPositions(ids, bandOf, seqIndex, rootId) {
    const pos = new Map();
    const rest = ids.filter((id) => id !== rootId)
      .sort((a, c) => bandOf(a) - bandOf(c) || (seqIndex.get(a) ?? 1e9) - (seqIndex.get(c) ?? 1e9) || a.localeCompare(c));
    if (rootId && ids.includes(rootId)) pos.set(rootId, { x: 0, y: 0 });
    const r0 = 210; const lapGrowth = 200; const arc = 190;
    let theta = -Math.PI / 2;
    rest.forEach((id) => {
      const r = r0 + (lapGrowth * (theta + Math.PI / 2)) / TAU;
      pos.set(id, { x: r * Math.cos(theta), y: r * Math.sin(theta) });
      theta += arc / r;
    });
    return pos;
  }

  // ------------------------------------------------------------------ cytoscape
  const cy = cytoscape({
    container: $('cy'),
    minZoom: 0.03, maxZoom: 3, boxSelectionEnabled: false,
    // Cytoscape's own wheel handling zooms on every wheel event, so a two-finger swipe on a Mac
    // trackpad zoomed. It is switched off here and replaced by the wheel listener below.
    userZoomingEnabled: false,
    style: [
      {
        selector: 'node',
        style: {
          'background-color': 'data(color)', 'border-color': 'data(border)', 'border-width': 1.5,
          shape: 'data(shape)', width: 'data(w)', height: 'data(h)', label: 'data(label)',
          'font-size': 'data(font)', 'font-family': 'ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif',
          color: '#d8dce3', 'text-valign': 'bottom', 'text-halign': 'center', 'text-margin-y': 4,
          'text-wrap': 'wrap', 'text-max-width': '130px',
          'text-background-color': '#14161a', 'text-background-opacity': 0.8, 'text-background-padding': '1px',
          'text-background-shape': 'roundrectangle', 'min-zoomed-font-size': 5,
        },
      },
      { selector: 'node.context, node.project, node.actor, node.usecase', style: { 'font-weight': 600 } },
      {
        selector: 'node:parent',
        style: {
          'background-opacity': 0.08, 'border-width': 1.5, 'border-opacity': 0.8, 'border-color': 'data(color)', shape: 'round-rectangle',
          'text-valign': 'top', 'text-halign': 'center', 'text-margin-y': -4, 'font-weight': 700, color: 'data(color)',
          padding: '14px', 'text-background-opacity': 0,
        },
      },
      { selector: 'node.context:parent', style: { 'font-size': 30, 'text-max-width': '600px' } },
      { selector: 'node.container:parent', style: { 'font-size': 17, 'text-max-width': '300px' } },
      { selector: 'node.root', style: { 'border-width': 4, 'border-color': '#f59e0b' } },
      {
        selector: 'edge',
        style: {
          width: 'data(width)', 'line-color': 'data(color)', 'line-style': 'data(line)',
          'target-arrow-color': 'data(color)', 'target-arrow-shape': 'data(arrow)', 'arrow-scale': 0.8,
          'curve-style': 'bezier', opacity: 0.65,
        },
      },
      { selector: 'edge.bad', style: { 'line-dash-pattern': [7, 4] } },
      { selector: 'edge.overview', style: { opacity: 0.32 } },
      { selector: 'edge.lib', style: { opacity: 0.35 } },
      { selector: 'edge.contains', style: { opacity: 0.85 } },
      {
        selector: 'edge.flow, edge.start',
        style: {
          opacity: 0.95, label: 'data(label)', 'font-size': 14, 'font-weight': 700, color: 'data(color)',
          'text-background-color': '#14161a', 'text-background-opacity': 0.92, 'text-background-padding': '2px',
          'text-background-shape': 'roundrectangle', 'text-border-width': 1, 'text-border-color': 'data(color)',
          'text-border-opacity': 0.6, 'z-index': 5,
        },
      },
      { selector: 'edge.curved', style: { 'curve-style': 'unbundled-bezier', 'control-point-distances': 'data(cpd)', 'control-point-weights': 0.5 } },
      { selector: 'node.focus', style: { 'font-size': 15, 'text-max-width': '180px' } },
      { selector: 'node.focus.component', style: { 'font-size': 14 } },
      { selector: 'edge.flow.branch', style: { width: 2.2, opacity: 0.8 } },
      { selector: 'node.anchor', style: { width: 1, height: 1, opacity: 0, label: '', events: 'no' } },
      { selector: 'edge.lifeline', style: { width: 1.2, 'line-color': '#3a414d', 'line-style': 'dashed', 'target-arrow-shape': 'none', opacity: 1, events: 'no' } },
      { selector: 'node.seqhead', style: { 'font-size': 19, 'text-max-width': '172px', 'text-valign': 'top', 'text-margin-y': -4, 'z-index': 20,
        'text-background-color': '#14161a', 'text-background-opacity': 0.92, 'text-background-padding': '3px' } },
      { selector: 'edge.seqmsg', style: { 'font-size': 17, 'curve-style': 'straight', 'arrow-scale': 1 } },
      { selector: 'edge.seqmsg.curved', style: { 'curve-style': 'unbundled-bezier' } },
      { selector: 'node.section', style: { width: 1, height: 1, opacity: 1, 'background-opacity': 0, 'border-width': 0, label: 'data(label)',
        'font-size': 16, 'font-weight': 700, color: '#8b93a1', 'text-valign': 'center', 'text-halign': 'center', events: 'no' } },
      { selector: '.dim', style: { opacity: 0.12 } },
      { selector: 'node.dim', style: { 'text-opacity': 0.35 } },
      { selector: '.faded', style: { opacity: 0.1 } },
      { selector: '.selfade', style: { opacity: 0.14 } },
      { selector: 'edge.hl', style: { opacity: 1, width: 3, 'z-index': 20 } },
      { selector: 'node.hl', style: { 'z-index': 20 } },
      { selector: 'node.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.5, 'overlay-padding': 7 } },
      { selector: 'edge.match', style: { 'overlay-color': '#facc15', 'overlay-opacity': 0.45, 'overlay-padding': 5 } },
      { selector: 'node.ghost', style: { 'border-style': 'dashed', 'background-opacity': 0.02 } },
      { selector: 'node.picked', style: { 'border-width': 4, 'border-color': '#f8fafc', 'overlay-color': '#38bdf8', 'overlay-opacity': 0.35, 'overlay-padding': 6 } },
      { selector: 'node:selected', style: { 'border-width': 4, 'border-color': '#f8fafc' } },
      { selector: 'edge:selected', style: { width: 4, opacity: 1, 'z-index': 30 } },
    ],
  });

  let VIEW = null;
  /**
   * The banner: lasting notes (a layout that needs a use case, references that match nothing)
   * stay while they hold; a hint (how to get around a long Sequence) clears after a few seconds
   * or as soon as the reader pans or zooms, whichever comes first.
   */
  let hintTimer = null;
  function showBanner(lasting, hint) {
    const b = $('banner');
    if (hintTimer) { clearTimeout(hintTimer); hintTimer = null; }
    const text = [lasting, hint].filter(Boolean).join(' ');
    b.textContent = text; b.classList.toggle('hidden', !text);
    if (!hint) return;
    const clear = () => {
      if (hintTimer) { clearTimeout(hintTimer); hintTimer = null; }
      cy.off('pan zoom', onMove);
      b.textContent = lasting; b.classList.toggle('hidden', !lasting);
    };
    let armed = false;
    const onMove = () => { if (armed) clear(); };
    setTimeout(() => { armed = true; }, 400); // the layout's own fit is not the reader moving
    cy.on('pan zoom', onMove);
    hintTimer = setTimeout(clear, 6000);
  }
  /**
   * Sequence: the column heads (the parts) stay pinned to the top edge while the reader scrolls
   * down the steps, so every arrow's two ends stay named.
   */
  let SEQ_PINNED = false;
  let SEQ_STATS = null; let LASTING = '';
  /** Where a Sequence starts: the whole diagram when it fits legibly; else readable at the
   *  top-left with a hint (Fit shows it all). Returns the hint, if any. */
  function seqStart() {
    const s = SEQ_STATS;
    cy.fit(undefined, 40);
    let hint = '';
    if (cy.zoom() < 0.6) {
      cy.zoom(0.6);
      const bb = cy.elements().boundingBox();
      cy.pan({ x: 40 - bb.x1 * 0.6, y: 30 - bb.y1 * 0.6 });
      hint = s ? `${s.rows} steps across ${s.parts} parts: drag or scroll to see the rest, or press Fit.` : '';
    } else if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); }
    pinHeads();
    return hint;
  }
  function pinHeads() {
    if (!SEQ_PINNED) return;
    const top = -cy.pan().y / cy.zoom();
    cy.nodes('.seqhead').forEach((n) => {
      const above = n.position('y') - n.boundingBox({ includeLabels: true }).y1; // label sits above the box
      n.position({ x: n.position('x'), y: Math.max(0, top + 8 / cy.zoom() + above) });
    });
  }
  let MERGED = new Map(); // cy edge id -> merged edge record

  function render() {
    lockEdges();
    writeHash();
    syncControls();
    $('showhidden').disabled = !HIDDEN.size;
    $('showhidden').textContent = HIDDEN.size ? `Show all hidden nodes (${HIDDEN.size})` : 'Show all hidden nodes';
    hideTip();
    const v = currentView();
    VIEW = v;
    let layout = state.layout;
    const banners = [];
    if (layout === 'flow' && !v.uc) {
      banners.push('Flow (left to right) follows one use case: pick one above. Showing Top-down tiered.');
      layout = 'tiered';
    }
    if (layout === 'sequence' && (!v.uc || state.dim || !v.uc.has_flowmap)) {
      banners.push(!v.uc ? 'Sequence needs one use case: pick one above. Showing Top-down tiered.'
        : state.dim ? 'Sequence needs the use case on its own: untick "dim the rest". Showing Top-down tiered.'
          : 'Sequence draws numbered steps, and this use case has no flowmap yet. Showing Top-down tiered.');
      layout = 'tiered';
    }
    if (v.uc && !v.uc.has_flowmap) banners.push('This use case has no flowmap yet, so there are no numbered steps: showing the code parts its file lists, grouped by level.');
    if (v.uc && v.uc.unresolved && v.uc.unresolved.length) banners.push(`${v.uc.unresolved.length} flowmap reference(s) match no node and are left out: ${v.uc.unresolved.join(', ')}`);
    MERGED = new Map();
    SEQ_PINNED = layout === 'sequence';
    let hint = '';
    if (layout === 'sequence') {
      const s = sequenceElements(v);
      cy.batch(() => { cy.elements().remove(); cy.add(s.els); });
      cy.resize();
      cy.nodes().positions((n) => s.pos.get(n.id()) || { x: 0, y: 0 });
      SEQ_STATS = s;
      hint = seqStart();
    }
    LASTING = banners.join(' ');
    showBanner(LASTING, hint);
    if (layout === 'sequence') {
      $('empty').classList.toggle('hidden', cy.nodes().nonempty());
      renderUcSummary(v);
      applySearch(false);
      if (SELECTED) reselect();
      return;
    }

    // Cluster: ancestors of the parts shown appear as dashed boxes so the grouping reads.
    const all = new Set(v.ids); const ghosts = new Set();
    if (layout === 'cluster' && v.uc && !state.dim) {
      for (const id of v.ids) {
        let p = (nodesById.get(id) || {}).parent;
        while (p && p !== 'project') { if (!all.has(p)) ghosts.add(p); p = (nodesById.get(p) || {}).parent; }
      }
      ghosts.forEach((g) => all.add(g));
    }
    const parentFor = (id) => {
      if (layout !== 'cluster') return undefined;
      let p = (nodesById.get(id) || {}).parent;
      while (p) { if (p !== 'project' && all.has(p)) return p; p = (nodesById.get(p) || {}).parent; }
      return undefined;
    };
    const ub = v.uc && !state.dim ? ucBands(v) : null;
    const bands = ub ? ub.bands : null;
    const els = [];
    for (const id of all) {
      const n = nodesById.get(id); if (!n) continue;
      const st = LEVELS[n.level] || LEVELS.container;
      const classes = [n.level];
      if (v.uc && state.dim && !v.touched.has(id)) classes.push('dim');
      if (ghosts.has(id)) classes.push('ghost');
      if (v.uc && id === v.uc.id) classes.push('root');
      const label = n.name;
      if (!v.whole) classes.push('focus');
      const big = v.uc && id === v.uc.id;
      els.push({
        group: 'nodes', classes: classes.join(' '),
        data: { id, label, level: n.level, color: st.color, border: st.border, shape: st.shape,
          w: big ? st.w * 1.3 : st.w, h: big ? st.h * 1.3 : st.h, font: st.font, parent: parentFor(id) },
      });
    }
    // Rows actually drawn (empty bands collapse), for deciding which edges skip rows.
    const rowRank = new Map();
    if (bands) {
      const rowFn = (id) => bands.get(id);
      const vals = [...new Set([...all].map(rowFn))].sort((a, b) => a - b);
      all.forEach((id) => rowRank.set(id, vals.indexOf(rowFn(id))));
    }
    const merged = mergeEdges(v.edges.filter((e) => all.has(e.source) && all.has(e.target)));
    // Row layouts: an edge that skips rows or goes back up bows out instead of running through the
    // nodes between (forward skips to the left, returns to the right); same-row edges bow too.
    // A bow's midpoint - where its step label sits - lands on the row it skips, half the bow out
    // from the centre line, so the bow is wide enough to clear that row's node names (up to 180px).
    const bow = merged.map((m) => {
      if (!(bands && layout === 'tiered' && m.type === 'flow')) return 0;
      const a = rowRank.get(m.source); const b = rowRank.get(m.target); const span = Math.abs(b - a);
      if (a === b) return -50;
      if (b < a) return 220 + 40 * span;
      if (span > 1) return 200 + 40 * span;
      return 0;
    });
    const cpds = spreadParallel(merged, bow);
    merged.forEach((m, i) => {
      const vis = edgeVisual(m);
      const eid = 'e' + i;
      MERGED.set(eid, m);
      const classes = [vis.cls];
      if (v.whole && m.type === 'interface' && !(v.uc && state.dim)) classes.push('overview');
      if (v.uc && state.dim && m.type !== 'flow' && m.type !== 'start') classes.push('dim');
      const cpd = cpds[i];
      if (cpd !== null) classes.push('curved');
      els.push({ group: 'edges', classes: classes.join(' '),
        data: { id: eid, source: m.source, target: m.target, cpd: cpd || 0, color: vis.color, line: vis.line, width: vis.width, arrow: vis.arrow, label: vis.label } });
    });
    cy.batch(() => { cy.elements().remove(); cy.add(els); });
    $('empty').classList.toggle('hidden', cy.nodes().nonempty());
    $('empty').textContent = 'Nothing to show with these settings.';
    renderUcSummary(v);
    runLayout(layout, v, ub);
    applySearch(false);
    if (SELECTED) reselect();
  }

  function runLayout(layout, v, ub) {
    const bands = ub ? ub.bands : null;
    cy.resize(); // the header and legend may have changed the canvas size since the last layout
    if (cy.nodes().empty()) return;
    const ids = cy.nodes().map((n) => n.id());
    const layEdges = cy.edges().map((e) => ({ source: e.source().id(), target: e.target().id(), flow: e.hasClass('flow') }));
    // Children sit near their parents in tiered/rings even when contains edges are not drawn.
    const inView = new Set(ids);
    const structural = containsEdges.filter((e) => inView.has(e.source) && inView.has(e.target) && e.source !== 'project')
      .map((e) => ({ source: e.source, target: e.target, flow: false }));
    const wholeBandT = (id) => (id === (v.uc && v.uc.id) ? -1 : (WHOLE_TIER[levelOf(id)] ?? 5));
    const wholeBandR = (id) => (id === (v.uc && v.uc.id) ? -1 : (WHOLE_RING[levelOf(id)] ?? 6));
    const bandOf = bands ? (id) => bands.get(id) ?? 99 : null;
    let pos = null;
    const aspect = Math.min(3, Math.max(0.8, cy.width() / Math.max(1, cy.height())));
    if (layout === 'cluster') {
      cy.layout({
        name: 'fcose', quality: 'default', randomize: true, animate: false, nodeDimensionsIncludeLabels: true,
        idealEdgeLength: () => 70, nodeRepulsion: () => 8000, edgeElasticity: () => 0.15, nestingFactor: 0.1,
        gravity: 0.5, gravityCompound: 2, gravityRangeCompound: 1.0, numIter: 4000, packComponents: true,
        ...(window.__fcoseOverride || {}),
        tile: true, tilingPaddingVertical: 20, tilingPaddingHorizontal: 20, fit: false,
      }).run();
    } else if (layout === 'flow') {
      cy.layout({ name: 'dagre', rankDir: 'LR', nodeSep: 26, rankSep: 110, edgeSep: 8, ranker: 'network-simplex',
        nodeDimensionsIncludeLabels: true, fit: false, animate: false }).run();
    } else if (layout === 'tiered') {
      pos = tieredPositions(ids, layEdges.concat(structural), bandOf || wholeBandT,
        bands ? { tierGap: 200, colGap: 210, aspect } : { aspect });
    } else if (layout === 'rings') {
      if (bands) {
        const seqIndex = new Map(v.uc.sequence.map((id, i) => [id, i]));
        pos = spiralPositions(ids, bandOf, seqIndex, v.uc.id);
      } else {
        pos = ringPositions(ids, layEdges.concat(structural), wholeBandR, { maxSubRings: 2, minArc: 120, ringGap: 190, subRingGap: 95 });
      }
    }
    if (pos) cy.nodes().positions((n) => pos.get(n.id()) || { x: 0, y: 0 });
    cy.fit(undefined, v.uc && !state.dim ? 55 : 30);
    if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); }
  }

  // ------------------------------------------------------------------ use-case summary box
  function renderUcSummary(v) {
    const box = $('ucsummary');
    if (!v.uc) { box.classList.add('hidden'); return; }
    const u = v.uc;
    const np = u.process_steps.length; const nd = u.data_steps.length;
    const counts = u.has_flowmap ? `${np} process step(s), ${nd} data step(s), ${u.sequence.length} parts touched` : `${u.sequence.length} parts listed (no flowmap yet)`;
    box.innerHTML = `<h2 title="Click to fold or unfold">${esc(u.name)} <span class="fold">▾</span></h2><div class="body"><p>${esc(u.summary || u.description)}</p><p class="muted" style="margin-top:4px;color:#6b7280;font-size:12px">${esc(counts)}</p></div>`;
    box.querySelector('h2').onclick = () => box.classList.toggle('folded');
    box.classList.remove('hidden');
  }

  // ------------------------------------------------------------------ hover
  let tipTimer = null;
  function hideTip() { if (tipTimer) clearTimeout(tipTimer); tipTimer = null; $('tip').classList.add('hidden'); }
  cy.on('mouseover', 'node', (evt) => {
    if (SELECTED) return;
    const hood = evt.target.closedNeighborhood().union(evt.target.ancestors()).union(evt.target.descendants())
      .union(stepEdgesOf(evt.target.id()));
    cy.elements().not(hood).addClass('faded');
    hood.addClass('hl');
  });
  // A short description of a node on hover: its plain-language phrase first (plain_labels.yaml), then its existing description.
  cy.on('mouseover', 'node', (evt) => {
    const n = nodesById.get(evt.target.id()); if (!n) return;
    const p = evt.renderedPosition || evt.target.renderedPosition();
    if (tipTimer) clearTimeout(tipTimer);
    tipTimer = setTimeout(() => {
      const desc = nodeSimple(n);
      const tip = $('tip');
      tip.innerHTML = `<div><b>${esc(n.name)}</b></div>${n.plain ? `<div class="plain">${esc(n.plain)}</div>` : ''}${desc ? `<div class="sub">${esc(desc)}</div>` : ''}`;
      tip.style.left = p.x + 'px'; tip.style.top = p.y + 'px';
      tip.classList.remove('hidden');
    }, 450);
  });
  cy.on('mouseout', 'node', () => { cy.elements().removeClass('faded hl'); hideTip(); });
  cy.on('mouseover', 'edge', (evt) => {
    const e = evt.target;
    if (!SELECTED) { cy.elements().not(e.union(e.connectedNodes())).addClass('faded'); e.addClass('hl'); }
    const p = evt.renderedPosition;
    if (tipTimer) clearTimeout(tipTimer);
    tipTimer = setTimeout(() => {
      const m = MERGED.get(e.id()); if (!m) return;
      const tip = $('tip');
      const e0 = m.members[0];
      const detail = m.type === 'flow' && e0.plain ? stepText(e0) : '';
      tip.innerHTML = `<div><b>${esc(edgeTitle(m))}</b></div>${detail ? `<div class="sub detail">${esc(detail)}</div>` : ''}<div class="sub">${esc(nameOf(m.source))} → ${esc(nameOf(m.target))}</div>`;
      tip.style.left = p.x + 'px'; tip.style.top = p.y + 'px';
      tip.classList.remove('hidden');
    }, 450);
  });
  cy.on('mouseout', 'edge', () => { cy.elements().removeClass('faded hl'); hideTip(); });
  cy.on('pan zoom tapstart', hideTip);
  // the drawing moving under a still pointer sends no mouseout: drop the hover highlight with it
  cy.on('pan zoom', () => { if (!SELECTED) cy.elements('.faded, .hl').removeClass('faded hl'); });
  cy.on('viewport', pinHeads);

  function edgeTitle(m) {
    const e = m.members[0];
    if (m.type === 'interface') return m.members.length > 1 ? `${m.members.length} interfaces${e.is_library ? ' (library)' : ''}: ${m.members.map(ifLabel).join(', ')}` : ifLabel(e);
    if (m.type === 'flow') return `Step ${stepName(e)} (${e.kind}${e.branch ? ', branch ' + e.branch : ''}): ${e.plain || stepText(e)}`;
    if (m.type === 'contains') return 'contains';
    if (m.type === 'start') return 'The use case starts here';
    return 'touched by this use case';
  }

  // ------------------------------------------------------------------ selection + detail panel
  let SELECTED = null; // {kind:'node'|'edge', id, key?}
  cy.on('tap', 'node', (evt) => select({ kind: 'node', id: evt.target.id() }));
  cy.on('tap', 'edge', (evt) => { const m = MERGED.get(evt.target.id()); select({ kind: 'edge', id: evt.target.id(), key: m && m.key }); });
  cy.on('tap', (evt) => { if (evt.target === cy) select(null); });

  function select(sel) {
    SELECTED = sel;
    cy.elements().unselect().removeClass('selfade faded hl picked');
    if (!sel) { $('detail').classList.add('hidden'); cy.resize(); return; }
    reselect();
    $('detail').classList.remove('hidden');
    cy.resize();
    renderDetail();
  }
  function reselect() {
    let el;
    if (SELECTED.kind === 'node') el = cy.getElementById(SELECTED.id);
    else {
      let eid = null;
      MERGED.forEach((m, id) => { if (m.key === SELECTED.key) eid = id; });
      el = eid ? cy.getElementById(eid) : cy.collection();
      if (eid) SELECTED.id = eid;
    }
    if (!el || el.empty()) return;
    el.select();
    let keep;
    if (el.isNode()) keep = el.closedNeighborhood().union(el.ancestors()).union(stepEdgesOf(el.id()));
    else {
      // In the Sequence diagram an arrow joins invisible anchors: its end parts are the column heads.
      const m = MERGED.get(el.id());
      const ends = m ? cy.getElementById(m.source).union(cy.getElementById(m.target)) : cy.collection();
      ends.addClass('picked');
      keep = el.union(el.connectedNodes()).union(ends).union(ends.ancestors());
    }
    cy.elements().not(keep).addClass('selfade');
  }
  /** In the Sequence diagram a step's arrow joins two invisible anchors; find the arrows by the parts they stand for. */
  function stepEdgesOf(id) {
    const es = cy.edges('.seqmsg').filter((ed) => { const m = MERGED.get(ed.id()); return !!m && (m.source === id || m.target === id); });
    return es.union(es.connectedNodes()).union(cy.getElementById('lle|' + id));
  }
  function focusNode(id) {
    if (HIDDEN.has(id)) { HIDDEN.delete(id); saveHidden(); render(); }
    let el = cy.getElementById(id);
    if (el.empty() && state.uc) { state.uc = ''; render(); el = cy.getElementById(id); }
    if (el.empty() && ucById.has(id)) { setUc(id); render(); el = cy.getElementById(id); }
    if (el.empty()) return;
    select({ kind: 'node', id });
    cy.animate({ center: { eles: el }, zoom: Math.max(cy.zoom(), 0.9) }, { duration: 250 });
  }

  function esc(s) { return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }
  /** A step's detailed text: `what` for a process step, `data` for a data step. Its `plain` phrase, when set, is shown above it. */
  const stepText = (e) => (e.kind === 'process' ? e.what : e.data) || e.what || e.data || '';
  /** A node's existing plain description (its `simple_description`; for a use case, its flowmap summary). */
  const nodeSimple = (n) => (n.level === 'usecase' ? ((ucById.get(n.id) || {}).summary || n.description) : (n.simple_description || n.description)) || '';
  const dot = (id) => `<span class="dot" style="background:${(LEVELS[levelOf(id)] || LEVELS.container).color}"></span>`;
  const nodeLink = (id) => `${dot(id)}<a class="node" data-node="${esc(id)}">${esc(nameOf(id))}</a>`;
  // Words shown for a status. The data keeps its own words (not-built, broken, not-deployed); the
  // page says them in plain terms and never shows the word "broken" (Stephen, 2026-09-28).
  const STATUS_TEXT = { 'not-built': 'not built yet', broken: 'not working yet', 'not-deployed': 'not deployed yet', live: 'working', complete: 'complete' };
  const statusTag = (s) => (s ? `<span class="st ${isBad(s) ? 'bad' : 'ok'}">${esc(STATUS_TEXT[String(s).toLowerCase()] || s)}</span>` : '');
  const evidenceHtml = (ev) => {
    if (!ev) return '';
    const list = Array.isArray(ev) ? ev : [ev];
    return `<div class="muted">Evidence: ${list.map((x) => `<code>${esc(x)}</code>`).join(', ')}</div>`;
  };

  function renderDetail() {
    const box = $('detail');
    let html = '<button class="close" title="Close">×</button>';
    if (SELECTED.kind === 'node') html += nodeDetail(SELECTED.id); else html += edgeDetail(MERGED.get(SELECTED.id));
    box.innerHTML = html;
    box.scrollTop = 0;
    box.querySelector('.close').onclick = () => select(null);
    box.querySelectorAll('button.hidenode').forEach((b) => { b.onclick = () => hideNode(b.dataset.hide); });
    box.querySelectorAll('a.node').forEach((a) => { a.onclick = () => focusNode(a.dataset.node); });
    box.querySelectorAll('a.uc').forEach((a) => { a.onclick = () => { state.uc = a.dataset.uc; onUcChange(); }; });
    // iter4: the graph editor (graphedit.js) adds its actions to the panel
    ROOT.dispatchEvent(new CustomEvent('g-detail', { detail: { kind: SELECTED.kind, id: SELECTED.id, node: SELECTED.kind === 'node' ? nodesById.get(SELECTED.id) : null, box } }));
  }

  function nodeDetail(id) {
    const n = nodesById.get(id);
    if (!n) return '';
    const st = LEVELS[n.level] || LEVELS.container;
    let h = `<h2>${esc(n.name)}</h2><span class="badge" style="background:${st.color}">${esc(st.label)}</span>`
      + ` <button type="button" class="hidenode" data-hide="${esc(id)}" title="Take this node and all its lines off the map until you press Show all hidden nodes">Hide this node</button>`;
    const simple = n.level === 'usecase' ? (ucById.get(id) || {}).summary : n.simple_description;
    if (n.plain) h += `<div class="plainline">${esc(n.plain)}</div>`;
    h += simple ? `<div class="simple">${esc(simple)}</div>` : '<div class="simple missing">No plain-language description yet (simple_description is not set in this file).</div>';
    if (n.description && n.description !== simple) h += `<h4>Description</h4><p>${esc(n.description)}</p>`;
    if (n.path) h += `<h4>File</h4><span class="path">${esc(n.path)}</span>`;
    // the code this node owns (its codedirs), linked to the repository's web view
    if (Array.isArray(n.code_files) && n.code_files.length) {
      const repo = (G.summary && G.summary.repo) || {};
      const href = (f) => {
        if (!repo.web) return '';
        const rel = String(f).replace(/^\{topdir\}\//, '');
        return `${repo.web}/blob/${encodeURIComponent(repo.branch || 'main')}/${(repo.prefix || '')}${rel}`;
      };
      const items = n.code_files.map((f) => {
        const label = esc(String(f).replace(/^\{topdir\}\//, ''));
        const u = href(f);
        return `<li>${u ? `<a class="codefile" href="${esc(u)}" target="_blank" rel="noopener">${label}</a>` : `<span class="path">${label}</span>`}</li>`;
      }).join('');
      h += `<h4>Code (${n.code_files.length}${n.code_files.length >= 60 ? '+' : ''})</h4><ul class="codefiles">${items}</ul>`;
    }
    if (Array.isArray(n.uncovered_files) && n.uncovered_files.length) {
      h += `<h4>Code no component owns (${n.uncovered_files.length})</h4><p class="muted">These files sit beside code its components own, but no component lists them — add them to a component's codedirs, or add a component for them.</p><ul class="codefiles">${n.uncovered_files.map((f) => `<li><span class="path">${esc(String(f).replace(/^\{topdir\}\//, ''))}</span></li>`).join('')}</ul>`;
    }
    const chain = []; let p = n.parent;
    while (p) { chain.unshift(p); p = (nodesById.get(p) || {}).parent; }
    if (chain.length) h += `<h4>Inside</h4><div class="chain">${chain.map(nodeLink).join('')}</div>`;
    const kids = G.nodes.filter((x) => x.parent === id);
    if (kids.length) h += `<h4>Contains (${kids.length})</h4><ul>${kids.map((k) => `<li>${nodeLink(k.id)}</li>`).join('')}</ul>`;

    if (n.level === 'usecase') h += usecaseSteps(id);

    // use cases that touch it
    const ucs = n.usecases || [];
    if (ucs.length) {
      h += `<h4>Use cases that pass through it (${ucs.length})</h4><ul>`;
      for (const t of ucs) {
        const u = ucById.get(t.usecase);
        const where = t.steps.length ? `step${t.steps.length > 1 ? 's' : ''} ${t.steps.join(', ')}` : t.order ? `listed ${t.order}${u && u.has_flowmap ? '' : ' (no flowmap yet)'}` : 'owns a part it needs';
        h += `<li><a class="uc" data-uc="${esc(t.usecase)}">${esc(u ? u.name : t.usecase)}</a> <span class="muted">- ${esc(where)}</span></li>`;
      }
      h += '</ul>';
    }
    if (VIEW && VIEW.uc && n.level !== 'usecase') {
      const mine = (flowsByUc.get(VIEW.uc.id) || []).filter((e) => e.source === id || e.target === id);
      if (mine.length) {
        h += `<h4>Its steps in this use case</h4>`;
        mine.sort((a, b) => (a.kind === b.kind ? 0 : a.kind === 'process' ? -1 : 1) || (a.step ?? 0) - (b.step ?? 0)).forEach((e) => { h += flowCard(e); });
      }
    }

    // interfaces
    const out = ifaceEdges.filter((e) => e.source === id); const inn = ifaceEdges.filter((e) => e.target === id);
    if (n.level === 'context' && (out.length || inn.length)) {
      h += `<p class="muted">These interfaces are declared in this context's own file (<code>${esc(n.path)}</code>), for code that sits directly in the folder and inside none of the containers listed above ${id === 'devops' ? '(here, chiefly the scripts in <code>devops/script/</code>)' : ''}. They are not rolled up from its containers; each container's own interfaces are shown on that container.</p>`;
    }
    h += ifaceSection('Interfaces out (this part calls, publishes or supplies)', out, 'target');
    h += ifaceSection('Interfaces in (other parts call or read this one)', inn, 'source');
    if (n.long_description) {
      h += `<details><summary>Long description</summary><div class="longdesc">${esc(n.long_description.replace(/\*\*/g, ''))}</div></details>`;
    }
    return h;
  }

  function ifaceSection(title, list, otherKey) {
    if (!list.length) return `<h4>${esc(title)}</h4><p class="muted">None recorded.</p>`;
    const rt = list.filter((e) => !e.is_library); const lib = list.filter((e) => e.is_library);
    const group = (arr) => {
      const by = new Map();
      arr.forEach((e) => { const k = e[otherKey]; if (!by.has(k)) by.set(k, []); by.get(k).push(e); });
      return [...by.entries()].sort((a, b) => nameOf(a[0]).localeCompare(nameOf(b[0]))).map(([other, es]) =>
        `<li>${nodeLink(other)}<ul>${es.map((e) => `<li>${esc(ifLabel(e))} <code class="muted">${esc(e.interface)}</code> <span class="muted">${esc(KIND_TEXT[e.kind] || e.kind || '')}</span>${isBad(e.status) ? statusTag(e.status) : ''}</li>`).join('')}</ul></li>`).join('');
    };
    let h = `<h4>${esc(title)} - ${rt.length} runtime${lib.length ? `, ${lib.length} library` : ''}</h4>`;
    if (rt.length) h += `<ul>${group(rt)}</ul>`;
    if (lib.length) h += `<details><summary>${lib.length} library link(s)</summary><ul>${group(lib)}</ul></details>`;
    return h;
  }

  function flowCard(e) {
    const text = stepText(e);
    return `<div class="card"><b>Step ${esc(stepName(e))}</b> <span class="muted">${esc(e.kind)} flow${e.branch ? ', branch ' + esc(e.branch) : ''}</span>${statusTag(e.status && e.status)}${e.stored ? ' <span class="muted">(stored here)</span>' : ''}
      <div>${nodeLink(e.source)} → ${nodeLink(e.target)}</div>
      ${e.plain ? `<div class="plainline">${esc(e.plain)}</div><div class="what detail">${esc(text)}</div>` : `<div class="what">${esc(text)}</div>`}
      ${e.via ? `<div class="muted">Via: <code>${esc(e.via)}</code></div>` : ''}${evidenceHtml(e.evidence)}</div>`;
  }

  function usecaseSteps(id) {
    const u = ucById.get(id); if (!u) return '';
    let h = `<h4>File</h4><span class="path">${esc(u.path)}</span>`;
    if (!u.has_flowmap) {
      return h + `<h4>Parts it lists (${u.codenodes.length})</h4><ul>${u.codenodes.map((c) => `<li>${nodeLink(c)}</li>`).join('')}</ul>`;
    }
    for (const [kind, title] of [['process', 'Process flow'], ['data', 'Data flow']]) {
      const list = (flowsByUc.get(id) || []).filter((e) => e.kind === kind);
      if (!list.length) continue;
      h += `<h4>${title} (${list.length} steps)</h4>`;
      list.forEach((e) => { h += flowCard(e); });
    }
    return h;
  }

  function edgeDetail(m) {
    if (!m) return '';
    let h = '';
    if (m.type === 'interface') {
      h += `<h2>${m.members.length > 1 ? `${m.members.length} interfaces` : esc(ifLabel(m.members[0]))}</h2>`;
      h += `<div>${nodeLink(m.source)} → ${nodeLink(m.target)}</div>`;
      for (const e of m.members) {
        h += `<div class="card"><b>${esc(ifLabel(e))}</b> <code class="muted">${esc(e.interface)}</code>${statusTag(e.status)}
          <div class="muted">${esc(KIND_TEXT[e.kind] || e.kind)}${e.is_library ? ', library (build time)' : ''}; ${esc(e.edge_kind || '')}${e.transport ? '; ' + esc(e.transport) : ''}</div>
          <div class="what">${esc(e.description || '')}</div>
          ${e.why ? `<div class="what" style="color:#991b1b">${esc(e.why)}</div>` : ''}
          ${e.interface_file ? `<div class="muted">Contract: <code>${esc(e.interface_file)}</code></div>` : ''}${evidenceHtml(e.evidence)}</div>`;
      }
    } else if (m.type === 'flow') {
      const e = m.members[0];
      const u = ucById.get(e.usecase);
      h += `<h2>Step ${esc(stepName(e))} - ${esc(e.kind)} flow</h2><p class="muted">Use case: <a class="uc" data-uc="${esc(e.usecase)}">${esc(u ? u.name : e.usecase)}</a></p>`;
      h += flowCard(e);
    } else if (m.type === 'contains') {
      h += `<h2>Contains</h2><p>${nodeLink(m.source)} contains ${nodeLink(m.target)}.</p>`;
    } else {
      const u = ucById.get(m.source);
      h += `<h2>${m.type === 'start' ? 'Where the use case starts' : 'Touched by a use case'}</h2><p><a class="uc" data-uc="${esc(m.source)}">${esc(u ? u.name : m.source)}</a> → ${nodeLink(m.target)}</p>`;
    }
    return h;
  }

  // ------------------------------------------------------------------ search
  /*
   * The drawing is a canvas, so the browser's own Find (Cmd+F) sees no text in it. This box searches
   * nodes AND edges instead: a node by its name, plain phrase, description or path; a use-case step by
   * its number ("step 60", "60", "D12"), plain phrase, what/data text, "via" and both end parts; an
   * interface link by its interface name and both end parts. Several words must all match (any field).
   * A word that is only a number (or D + number) matches a step's number exactly, so "60" is step 60
   * and not step 160. Cmd+F / Ctrl+F and "/" jump to the box.
   */
  const NUM_TOKEN = /^d?\d+\*?$/;
  const SEARCH_INDEX = [];
  for (const n of G.nodes) {
    SEARCH_INDEX.push({ type: 'node', n, fields: [['name', n.name], ['plain', n.plain], ['description', n.simple_description],
      ['description', n.description], ['path', n.id]].filter((f) => f[1]).map((f) => [f[0], String(f[1]), String(f[1]).toLowerCase()]) });
  }
  for (const e of G.edges) {
    if (e.type !== 'flow' && e.type !== 'interface') continue;
    const f = e.type === 'flow'
      ? [['plain', e.plain], ['what', e.what], ['data', e.data], ['via', e.via], ['from', nameOf(e.source)], ['to', nameOf(e.target)]]
      : [['interface', e.interface], ['from', nameOf(e.source)], ['to', nameOf(e.target)]];
    const ent = { type: e.type, e, fields: f.filter((x) => x[1]).map((x) => [x[0], String(x[1]), String(x[1]).toLowerCase()]) };
    if (e.type === 'flow') ent.stepWords = ['step', stepName(e).toLowerCase(), String(e.step)];
    SEARCH_INDEX.push(ent);
  }
  const ENDPOINT_FIELDS = new Set(['from', 'to']);
  const searchTokens = (q) => q.trim().toLowerCase().split(/\s+/).filter(Boolean);

  /** Every raw edge id drawn right now (merged interface edges list all their members). */
  function drawnEdgeIds() {
    const s = new Set();
    MERGED.forEach((m) => m.members.forEach((x) => { if (x.id) s.add(x.id); }));
    return s;
  }
  /** Why an edge is not drawn in the current view, in words; '' when it is drawn. */
  function hiddenReason(ent, drawn) {
    const e = ent.e;
    if (drawn.has(e.id)) return '';
    if (e.type === 'flow') {
      if (e.usecase !== state.uc) return 'in ' + ((ucById.get(e.usecase) || {}).name || e.usecase);
      if (state.flow !== 'both' && state.flow !== e.kind) return e.kind + ' steps are off';
      if (state.branch && e.branch && e.branch !== state.branch) return 'on branch ' + e.branch;
      if (!state.showBroken && isBad(e.status)) return '"not built yet" is off';
      return 'not drawn in this layout';
    }
    if (!state.showIface) return '"interfaces" is off';
    if (e.is_library && !state.showLib) return '"libraries" is off';
    if (!state.showBroken && isBad(e.status)) return '"not built yet" is off';
    if (state.uc) return 'outside this use case';
    return 'not drawn in this view';
  }

  /** Scored matches: { nodes: [...], edges: [...] }, each item { ent, hits, hidden }. */
  function matchesFor(q) {
    const toks = searchTokens(q);
    const empty = { nodes: [], edges: [], toks };
    if (!toks.length || (q.trim().length < 2 && !NUM_TOKEN.test(toks[0]))) return empty;
    const drawn = drawnEdgeIds();
    const nodes = []; const edges = [];
    for (const ent of SEARCH_INDEX) {
      let primary = false; let stepExact = false; let ok = true;
      for (const t of toks) {
        if (ent.stepWords && NUM_TOKEN.test(t)) {
          if (ent.stepWords.includes(t.replace('*', ''))) { stepExact = true; primary = true; continue; }
          ok = false; break;
        }
        if (ent.stepWords && ent.stepWords[0].includes(t)) continue; // "step"
        const f = ent.fields.find((x) => x[2].includes(t));
        if (!f) { ok = false; break; }
        if (!ENDPOINT_FIELDS.has(f[0])) primary = true;
      }
      if (!ok) continue;
      if (ent.type === 'node') {
        const inName = toks.every((t) => ent.n.name.toLowerCase().includes(t));
        nodes.push({ ent, hidden: cy.getElementById(ent.n.id).empty(), score: [inName ? 0 : 1, LEVEL_ORDER.indexOf(ent.n.level)] });
      } else {
        const e = ent.e;
        const hidden = hiddenReason(ent, drawn);
        // With the Edges toggles locked off, interface links and not-built/broken steps can never be
        // shown, so a result pointing at one would lead nowhere: leave it out.
        if (EDGE_TOGGLES_LOCKED && (e.type !== 'flow' || isBad(e.status))) continue;
        const here = e.type === 'flow' && e.usecase === state.uc;
        const typeRank = e.type === 'flow' ? 0 : e.is_library ? 2 : 1;
        edges.push({ ent, hidden, score: [stepExact ? 0 : 1, hidden ? 1 : 0, here ? 0 : 1, primary ? 0 : 1, typeRank,
          e.kind === 'data' ? 1 : 0, e.step ?? 1e9] });
      }
    }
    const cmp = (a, b) => { for (let i = 0; i < a.score.length; i++) if (a.score[i] !== b.score[i]) return a.score[i] - b.score[i]; return 0; };
    nodes.sort(cmp); edges.sort(cmp);
    return { nodes, edges, toks };
  }

  /** Escape text and wrap every occurrence of a search word in <mark>. */
  function hi(text, toks) {
    const s = String(text == null ? '' : text); const low = s.toLowerCase(); const r = [];
    toks.forEach((t) => { const w = t.replace('*', ''); if (!w) return; let i = low.indexOf(w); while (i >= 0) { r.push([i, i + w.length]); i = low.indexOf(w, i + w.length); } });
    if (!r.length) return esc(s);
    r.sort((a, b) => a[0] - b[0]);
    let out = ''; let at = 0;
    for (const [a, b] of r) { if (b <= at) continue; const from = Math.max(a, at); out += esc(s.slice(at, from)) + '<mark>' + esc(s.slice(from, b)) + '</mark>'; at = b; }
    return out + esc(s.slice(at));
  }
  const trunc = (s, n) => { s = String(s || ''); return s.length > n ? s.slice(0, n - 1) + '…' : s; };
  /** The words not already visible in the row, shown in the field that holds them: "what: …text…". */
  function matchedLine(ent, shown, toks) {
    const seen = shown.toLowerCase();
    const missing = toks.filter((t) => !NUM_TOKEN.test(t) && t !== 'step' && !seen.includes(t));
    if (!missing.length) return '';
    const f = ent.fields.find((x) => missing.some((t) => x[2].includes(t)) && !seen.includes(x[2]));
    if (!f) return '';
    const t = missing.find((w) => f[2].includes(w)); const i = f[2].indexOf(t);
    const a = Math.max(0, i - 40); const b = Math.min(f[1].length, i + t.length + 60);
    return `<div class="sm">${esc(f[0])}: ${a ? '…' : ''}${hi(f[1].slice(a, b), toks)}${b < f[1].length ? '…' : ''}</div>`;
  }
  function edgeRowTitle(e) {
    return e.type === 'flow' ? `Step ${stepName(e)}${e.branch ? '*' : ''} · ${e.kind} · ${nameOf(e.source)} → ${nameOf(e.target)}`
      : `${ifLabel(e)} · ${e.is_library ? 'library' : 'interface'} · ${nameOf(e.source)} → ${nameOf(e.target)}`;
  }

  const SEARCH_CAP = { nodes: 8, edges: 16 };
  let SEARCH_ROWS = []; let SEARCH_ACTIVE = 0;
  function applySearch(showList) {
    const q = $('search').value;
    const res = matchesFor(q);
    const toks = res.toks;
    cy.elements().removeClass('match');
    res.nodes.forEach((r) => cy.getElementById(r.ent.n.id).addClass('match'));
    if (res.edges.length) {
      const hit = new Set(res.edges.map((r) => r.ent.e.id));
      MERGED.forEach((m, id) => { if (m.members.some((x) => hit.has(x.id))) cy.getElementById(id).addClass('match'); });
    }
    const list = $('searchlist');
    SEARCH_ROWS = [...res.nodes.slice(0, SEARCH_CAP.nodes), ...res.edges.slice(0, SEARCH_CAP.edges)];
    SEARCH_ACTIVE = 0;
    if (!showList || !q.trim()) { list.classList.add('hidden'); return res; }
    let h = ''; let i = 0;
    const more = (n) => (n > 0 ? `<div class="more">${n} more… type another word to narrow</div>` : '');
    if (res.nodes.length) {
      h += `<div class="grp">Nodes (${res.nodes.length})</div>`;
      res.nodes.slice(0, SEARCH_CAP.nodes).forEach((r) => {
        const n = r.ent.n; const shown = n.name + ' ' + (n.plain || '');
        h += `<div class="row" data-i="${i++}">${dot(n.id)}<div class="txt"><div><b>${hi(n.name, toks)}</b> <span class="muted">${esc(n.level)}</span></div>`
          + (n.plain ? `<div class="sm">${hi(trunc(n.plain, 110), toks)}</div>` : '') + matchedLine(r.ent, shown, toks)
          + (r.hidden ? '<div class="hid">not in this view — click to show</div>' : '') + '</div></div>';
      });
      h += more(res.nodes.length - SEARCH_CAP.nodes);
    }
    if (res.edges.length) {
      h += `<div class="grp">Steps / arrows (${res.edges.length})</div>`;
      res.edges.slice(0, SEARCH_CAP.edges).forEach((r) => {
        const e = r.ent.e; const title = edgeRowTitle(e);
        const sub = e.type === 'flow' ? (e.plain || stepText(e)) : (e.description || '');
        const col = e.type === 'flow' ? (e.kind === 'process' ? '#1d4ed8' : '#ea580c') : (KIND_COLOR[e.kind] || '#cbd5e1');
        const ucName = e.type === 'flow' && e.usecase !== state.uc ? (ucById.get(e.usecase) || {}).name || e.usecase : '';
        h += `<div class="row" data-i="${i++}"><span class="bar" style="background:${isBad(e.status) ? BAD : col}"></span><div class="txt"><div><b>${hi(title, toks)}</b></div>`
          + (sub ? `<div class="sm">${hi(trunc(sub, 110), toks)}</div>` : '')
          + matchedLine(r.ent, title + ' ' + trunc(sub, 110), toks)
          + (ucName ? `<div class="muted">${esc(ucName)}</div>` : '')
          + (r.hidden ? `<div class="hid">hidden in this view (${esc(r.hidden)}) — click to show</div>` : '') + '</div></div>';
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
    if (r.ent.type === 'node') focusNode(r.ent.n.id); else focusEdge(r.ent.e);
  }

  /**
   * Show one raw edge (a flow step or an interface link): turn on only what the view needs to draw
   * it - its use case, its flow kind, its branch, interfaces / libraries / broken links - then select
   * it, fade the rest, centre on it and open its card, as clicking it does.
   */
  function focusEdge(e) {
    const drawnEid = () => { let eid = null; MERGED.forEach((m, id) => { if (!eid && m.members.some((x) => x.id === e.id)) eid = id; }); return eid; };
    let eid = drawnEid();
    if (!eid) {
      if (e.type === 'flow') {
        const moved = state.uc !== e.usecase;
        if (moved) setUc(e.usecase);
        if (state.flow !== 'both' && state.flow !== e.kind) state.flow = moved ? e.kind : 'both';
        if (state.branch && e.branch && e.branch !== state.branch) state.branch = '';
      } else {
        const both = () => { const v = currentView(); return v.idset.has(e.source) && v.idset.has(e.target); };
        state.showIface = true;
        if (state.uc && !both()) setUc('');
        state.showIface = true;
        if (e.is_library) state.showLib = true;
        // The Sequence diagram draws numbered steps only, never interface links.
        if (state.layout === 'sequence') state.layout = 'tiered';
      }
      if (isBad(e.status)) state.showBroken = true;
      render();
      eid = drawnEid();
      if (!eid && e.type === 'interface' && state.uc) {
        setUc(''); state.showIface = true; if (e.is_library) state.showLib = true;
        render();
        eid = drawnEid();
      }
    }
    if (!eid) { $('banner').textContent = 'That link could not be drawn in any view.'; $('banner').classList.remove('hidden'); return; }
    const m = MERGED.get(eid);
    select({ kind: 'edge', id: eid, key: m.key });
    const el = cy.getElementById(eid);
    cy.animate({ center: { eles: el }, zoom: Math.min(cy.maxZoom(), Math.max(cy.zoom(), 1)) }, { duration: 300 });
  }

  $('search').addEventListener('input', () => applySearch(true));
  $('search').addEventListener('focus', () => applySearch(true));
  $('search').addEventListener('blur', () => $('searchlist').classList.add('hidden'));
  $('search').addEventListener('keydown', (ev) => {
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      ev.preventDefault();
      if ($('searchlist').classList.contains('hidden')) applySearch(true);
      if (!SEARCH_ROWS.length) return;
      SEARCH_ACTIVE = (SEARCH_ACTIVE + (ev.key === 'ArrowDown' ? 1 : -1) + SEARCH_ROWS.length) % SEARCH_ROWS.length;
      markActive(true);
    } else if (ev.key === 'Enter') {
      ev.preventDefault();
      if (SEARCH_ROWS.length) pickResult(SEARCH_ACTIVE);
    } else if (ev.key === 'Escape') {
      $('search').value = ''; applySearch(false);
    }
  });
  // The drawing is a canvas the browser's Find cannot read: Cmd+F / Ctrl+F (and "/") open this search instead.
  onDoc('keydown', (ev) => {
    if (!ROOT.offsetParent) return; // the tab is hidden
    const typing = /^(INPUT|SELECT|TEXTAREA)$/.test((document.activeElement || {}).tagName || '');
    const find = (ev.metaKey || ev.ctrlKey) && !ev.altKey && !ev.shiftKey && (ev.key === 'f' || ev.key === 'F');
    if (find || (ev.key === '/' && !typing && !ev.metaKey && !ev.ctrlKey)) {
      ev.preventDefault();
      $('search').focus(); $('search').select();
      applySearch(true);
    }
  });

  // ------------------------------------------------------------------ controls
  function fillPicker() {
    const sel = $('ucpick');
    sel.innerHTML = '<option value="">Whole repository</option>' + G.usecases.map((u) =>
      `<option value="${esc(u.id)}">${esc(u.name)}${u.has_flowmap ? '' : ' (no flowmap yet)'}</option>`).join('');
  }
  function syncControls() {
    $('ucpick').value = state.uc;
    const uc = state.uc ? ucById.get(state.uc) : null;
    ROOT.querySelectorAll('.ucopt').forEach((el) => el.classList.toggle('hidden', !uc));
    ROOT.querySelectorAll('.wholeopt').forEach((el) => el.classList.toggle('hidden', !!uc));
    $('dim').checked = state.dim;
    for (const k of ['showIface', 'showLib', 'showContains', 'showBroken', 'showUsecases']) $(k).checked = state[k];
    const forced = containsForced();
    $('showContains').checked = state.showContains || forced;
    $('showContains').disabled = forced;
    $('showContains').parentElement.title = forced
      ? 'Always on for the whole repository in Top-down tiered and Inside-out rings: each context is linked to its containers, each container to its components'
      : 'Parent to child links (context to container, container to component)';
    ROOT.querySelectorAll('#g-layouts button').forEach((b) => {
      b.classList.toggle('on', b.dataset.layout === state.layout);
      if (b.dataset.layout === 'sequence') b.disabled = !uc || state.dim;
      if (b.dataset.layout === 'flow') b.disabled = !uc;
    });
    ROOT.querySelectorAll('#g-flowkind button').forEach((b) => b.classList.toggle('on', b.dataset.flow === state.flow));
    const br = $('branch');
    const branches = uc ? uc.branches : [];
    br.classList.toggle('hidden', !branches.length);
    br.innerHTML = '<option value="">All branches</option>' + branches.map((b) => `<option value="${esc(b)}">Branch: ${esc(b)}</option>`).join('');
    br.value = branches.includes(state.branch) ? state.branch : '';
  }
  /** Switch the use case and the settings that go with it, without drawing (render() draws). */
  function setUc(uc) {
    state.uc = uc;
    state.branch = '';
    // A use case is a chain of steps: interface edges between its parts would drown the numbered steps.
    state.showIface = !state.uc;
    if (!state.uc && (state.layout === 'sequence' || state.layout === 'flow')) state.layout = 'cluster';
    if (state.uc && ucById.get(state.uc).has_flowmap && state.layout === 'cluster') state.layout = 'sequence';
    SELECTED = null; $('detail').classList.add('hidden'); cy.resize();
  }
  function onUcChange() {
    setUc(state.uc);
    render();
  }
  $('ucpick').addEventListener('change', (ev) => { state.uc = ev.target.value; onUcChange(); });
  $('dim').addEventListener('change', (ev) => { state.dim = ev.target.checked; if (state.dim && state.layout === 'sequence') state.layout = 'tiered'; render(); });
  for (const k of ['showIface', 'showLib', 'showContains', 'showBroken', 'showUsecases']) {
    $(k).addEventListener('change', (ev) => { state[k] = ev.target.checked; render(); });
  }
  ROOT.querySelectorAll('#g-layouts button').forEach((b) => b.addEventListener('click', () => { state.layout = b.dataset.layout; render(); }));
  ROOT.querySelectorAll('#g-flowkind button').forEach((b) => b.addEventListener('click', () => { state.flow = b.dataset.flow; render(); }));
  $('branch').addEventListener('change', (ev) => { state.branch = ev.target.value; render(); });
  // Wheel and trackpad. Browsers deliver a trackpad pinch as a wheel event with ctrlKey set, and a
  // two-finger swipe as a plain wheel event carrying deltaX/deltaY. Pinch (or ctrl/cmd + mouse
  // wheel) zooms about the pointer; everything else pans the drawing, because the page itself never
  // scrolls - the drawing fills the space between the header and the legend. A plain mouse wheel
  // therefore pans up and down; hold ctrl or cmd to zoom with it.
  const wheelPx = (ev, d) => d * (ev.deltaMode === 1 ? 16 : ev.deltaMode === 2 ? $('cy').clientHeight : 1);
  $('cy').addEventListener('wheel', (ev) => {
    ev.preventDefault();
    const box = $('cy').getBoundingClientRect();
    const at = { x: ev.clientX - box.left, y: ev.clientY - box.top };
    if (ev.ctrlKey || ev.metaKey) {
      if (inGesture) return; // Safari: the gesture events below already zoom for this pinch
      // A pinch sends small deltas (a few px); a mouse notch sends ~100. Capping at 25 keeps one
      // notch to about 1.3x instead of 2.7x.
      const d = Math.max(-25, Math.min(25, wheelPx(ev, ev.deltaY)));
      const z = Math.min(cy.maxZoom(), Math.max(cy.minZoom(), cy.zoom() * Math.exp(-d * 0.01)));
      cy.zoom({ level: z, renderedPosition: at });
    } else {
      let dx = wheelPx(ev, ev.deltaX), dy = wheelPx(ev, ev.deltaY);
      if (ev.shiftKey && !dx) { dx = dy; dy = 0; } // shift + mouse wheel scrolls sideways, as on any page
      cy.panBy({ x: -dx, y: -dy });
    }
  }, { passive: false });
  // Safari also reports a pinch as gesture events; left alone they zoom the whole page.
  let gestureStart = 1;
  let inGesture = false;
  $('cy').addEventListener('gesturestart', (ev) => { ev.preventDefault(); inGesture = true; gestureStart = cy.zoom(); });
  $('cy').addEventListener('gesturechange', (ev) => {
    ev.preventDefault();
    const box = $('cy').getBoundingClientRect();
    const z = Math.min(cy.maxZoom(), Math.max(cy.minZoom(), gestureStart * ev.scale));
    cy.zoom({ level: z, renderedPosition: { x: ev.clientX - box.left, y: ev.clientY - box.top } });
  });
  $('cy').addEventListener('gestureend', (ev) => { ev.preventDefault(); inGesture = false; });
  $('fit').addEventListener('click', () => cy.animate({ fit: { eles: cy.elements(), padding: 30 } }, { duration: 250 }));
  $('relayout').addEventListener('click', () => render());
  $('showhidden').addEventListener('click', () => showAllHidden());
  window.addEventListener('hashchange', () => {
    // a link to another project's graph is the app's to handle (it switches
    // project and redraws); this drawing must not read, or rewrite, its keys
    const hp = new URLSearchParams(location.hash.slice(1)).get('p');
    if (hp && CTX.project && hp !== CTX.project) return;
    if (!ROOT.offsetParent) return; // the tab is hidden
    readHash(); render();
  }, { signal: SIGNAL });

  // ------------------------------------------------------------------ legend + summary line
  function renderLegend() {
    const lineSvg = (x) => `<svg width="30" height="8"><line x1="1" y1="4" x2="${x.noArrow ? 29 : 23}" y2="4" stroke="${x.color}" stroke-width="${x.thick ? 3 : 2}"
      ${x.line === 'dashed' ? 'stroke-dasharray="5 3"' : x.line === 'dotted' ? 'stroke-dasharray="1.5 2.5"' : ''}/>${x.noArrow ? '' : `<polygon points="23,0 30,4 23,8" fill="${x.color}"/>`}</svg>`;
    const nodes = Object.entries(LEVELS).map(([k, s]) => `<div class="item"><span class="sw ${k === 'actor' ? 'round' : k === 'usecase' ? 'diamond' : ''}" style="background:${s.color};border-color:${s.border}"></span>${esc(s.label)}</div>`).join('');
    const edges = EDGE_LEGEND.map((x) => `<div class="item">${lineSvg(x)}${esc(x.label)}</div>`).join('');
    const note = 'An arrow from A to B is a call - a process step, or a data step carrying data - from A to B. '
      + 'The reply from B back to A is implied and not drawn. Kafka is the one exception to following the call: an arrow into Kafka is a service publishing a message (the producer), and an arrow out of Kafka is a service receiving one (the consumer), even though the consumer polls. Several steps between the same two parts each get their own curve and label.';
    $('legend').innerHTML = `<button type="button" id="g-legendToggle" class="legendtoggle" aria-expanded="false">Legend</button>`
      + `<div class="legendbody"><div class="row"><span class="lbl">Nodes</span>${nodes}</div><div class="row"><span class="lbl">Edges</span>${edges}</div>`
      + `<div class="row note"><span class="lbl">Note</span><span>${esc(note)}</span></div></div>`;
    // Collapsed by default to give the drawing the room (Stephen, 2026-09-28); the viewer's choice
    // is remembered in this browser only.
    let open = false;
    try { open = localStorage.getItem('iter4.graph.legendOpen') === '1'; } catch (e) { /* storage blocked */ }
    const apply = () => {
      $('legend').classList.toggle('collapsed', !open);
      $('legendToggle').setAttribute('aria-expanded', String(open));
      $('legendToggle').textContent = open ? 'Legend \u25BE (hide)' : 'Legend \u25B8 (show)';
      if (typeof cy !== 'undefined') cy.resize();
    };
    $('legendToggle').addEventListener('click', () => {
      open = !open;
      try { localStorage.setItem('iter4.graph.legendOpen', open ? '1' : '0'); } catch (e) { /* storage blocked */ }
      apply();
    });
    apply();
  }
  function renderSummary() {
    const c = G.summary.counts;
    const lv = c.nodes_per_level || {};
    const when = G.summary.generated_at ? new Date(G.summary.generated_at).toLocaleString() : '';
    const warn = (G.summary.warnings || []).length;
    const s = $('summary');
    s.textContent = `${lv.context || 0} contexts · ${lv.container || 0} containers · ${lv.component || 0} components · ${lv.actor || 0} actors · `
      + `${(c.edges_per_type || {}).interface || 0} interface links (${c.interface_edges_library || 0} library) · `
      + `${c.usecases} use cases (${c.usecases_with_flowmap} with a flowmap) · built ${when}${G.summary.git_commit ? ' at ' + G.summary.git_commit : ''}`
      + (warn ? ` · ${warn} build warning(s)` : '');
    if (warn) s.title = G.summary.warnings.join('\n');
  }

  readHash();
  fillPicker();
  renderLegend();
  renderSummary();
  render();
  window.__usecaseMap = { cy, state, render }; // for scripted checks (same name as the reference)
  // the app's "open in the graph" links (Intro slides): exact id, else a name
  // or path containing the query; the whole-repository view if needed
  // the editor's context menu: what a drawn edge stands for (merged interfaces)
  window.IterGraph.edgeInfo = (id) => (MERGED ? MERGED.get(id) : null);
  window.IterGraph.hide = (id) => hideNode(id);
  /** The canvas settles its size after the first layout: fit again, except a Sequence, which
   *  keeps its readable start (top-left, names legible) and its pinned heads. */
  window.IterGraph.settle = () => {
    cy.resize();
    if (SEQ_PINNED) { showBanner(LASTING, seqStart()); return; }
    cy.fit(undefined, VIEW && VIEW.uc && !state.dim ? 55 : 30);
    if (cy.zoom() > 1.4) { cy.zoom(1.4); cy.center(); }
  };
  window.IterGraph.focus = (q) => {
    const k = String(q || '').toLowerCase();
    if (!k) return false;
    const all = [...nodesById.values()];
    const hit = all.find((n) => n.id.toLowerCase() === k)
      || all.find((n) => String(n.name || '').toLowerCase() === k)
      || all.find((n) => String(n.name || '').toLowerCase().includes(k))
      || all.find((n) => n.id.toLowerCase().includes(k));
    if (!hit) return false;
    focusNode(hit.id);
    return true;
  };
  return cy;
  }

}());
