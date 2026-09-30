/*
 * iter4 Project graph editor: build a project from the map (Phase 2 R14,
 * Phase 3 D5/D6). A file-changing edit is accepted at once as a pending
 * datasync row (POST /api/projects/{p}/graph/edits): the first engine serving
 * the project picks it up on its next heartbeat, writes the files, commits
 * just those, pushes and re-syncs the map, which this page then redraws. A
 * test run is a `test` work item instead. Nothing here writes files.
 *
 *   toolbar:  sync indicator · + Context · + Global object · Edit a global object · Connect · Run tests
 *   node:     right-click or ⌘/ctrl-click, on every kind of node:
 *             code node → Add child node · Add edge (draw it) · Define tests · Run its tests · Hide
 *             actor     → Add edge (to a part it uses) · Edit actor · Hide
 *             use case  → Add edge (to a part it needs) · Queue the usecase agent · Edit its text · Hide
 *             project   → + Context · + Actor · + Global object
 *   edge:     right-click → Remove edge (with a reason): ownership, interface,
 *             an actor's use, or a part a use case needs
 *   panel:    the same node actions under the node's title
 *
 * Any level may own any level (a context may own a context). Tests are
 * defined before code on purpose (TDD): the `test` agent writes the scripts,
 * they run red, and the sweep turns red groups into work items.
 */
(function () {
  'use strict';
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  let S = null; // {root, G, ctx, full, byId}
  let onDetail = null; // the one g-detail listener (the tab element outlives each drawing)
  const LEVELS = ['context', 'container', 'component'];
  const NEXT = { context: 'container', container: 'component', component: 'component' };
  const isCode = (n) => n && LEVELS.includes(n.level);
  const fileOf = (n) => (n && n.path ? '{topdir}/' + n.path : '');

  window.IterGraphEdit = {
    /** Called after each draw: G is the /graph/view JSON, ctx the app's graph context. */
    attach(root, G, ctx) {
      S = { root, G, ctx, full: null, byId: new Map(G.nodes.map((n) => [n.id, n])) };
      const bar = root.querySelector('#g-editbar');
      if (!bar) return;
      if (ctx.myRole === 'viewer') { bar.innerHTML = '<span class="kv">read-only (viewer)</span>'; return; }
      bar.innerHTML = '<span class="g-sync" id="g-sync" title="graph edits waiting for an engine"></span>' + [
        ['ctx', '+ Context', 'A new top-level area (data, comms, auth…) with its requirement and test files'],
        ['actor', '+ Actor', 'A person or outside program at the edge of the map (a developer, an operator, a payment provider)'],
        ['global', '+ Global object', 'A project-wide business or technical requirement, or a use case'],
        ['editglobal', 'Edit a global object', 'Change the text of a bizreq, techreq, use case or interface'],
        ['connect', 'Connect', 'Record an interface between two parts: one uses what the other provides'],
        ['tests', 'Run tests', 'Queue a one-off run of a test group'],
      ].map(([k, t, tip]) => `<button class="plain" data-edit="${k}" title="${esc(tip)}">${esc(t)}</button>`).join('')
        + '<span class="kv g-hint" title="right-click (or ⌘/ctrl-click) a node or an edge for its actions">right-click a node for more</span>';
      bar.querySelectorAll('[data-edit]').forEach((b) => { b.onclick = () => openForm(b.dataset.edit, null); });
      if (onDetail) root.removeEventListener('g-detail', onDetail);
      onDetail = (ev) => decorate(ev.detail);
      root.addEventListener('g-detail', onDetail);
      wireMenu();
      refreshSync();
    },
  };

  // ------------------------------------------------------------------ context menu
  function cyNow() { const c = window.IterGraph && IterGraph.current; return c && c.cy; }
  let keyWired = false;
  function wireMenu() {
    const cy = cyNow();
    if (!cy) return;
    const at = (e) => e.renderedPosition || (e.target.isEdge && e.target.isEdge() ? e.target.renderedMidpoint() : e.target.renderedPosition());
    cy.on('cxttap', 'node', (e) => { const n = S.byId.get(e.target.id()); if (n) nodeMenu(n, at(e)); });
    cy.on('tap', 'node', (e) => {
      if (DRAW) { finishDraw(e.target.id()); return; }
      const oe = e.originalEvent || {};
      if (oe.metaKey || oe.ctrlKey) { const n = S.byId.get(e.target.id()); if (n) nodeMenu(n, at(e)); }
    });
    cy.on('cxttap', 'edge', (e) => edgeMenu(e.target.id(), at(e)));
    cy.on('tap', (e) => { if (e.target === cy) { closeMenu(); if (DRAW) cancelDraw(); } });
    const wrap = S.root.querySelector('#g-cywrap');
    if (wrap) wrap.addEventListener('contextmenu', (ev) => ev.preventDefault());
    if (!keyWired) {
      keyWired = true;
      document.addEventListener('keydown', (ev) => { if (ev.key === 'Escape' && S) { closeMenu(); if (DRAW) cancelDraw(); } });
    }
  }
  function menuEl() {
    let m = S.root.querySelector('#g-menu');
    if (!m) {
      m = document.createElement('div');
      m.id = 'g-menu';
      m.className = 'g-menu hidden';
      (S.root.querySelector('#g-cywrap') || S.root).appendChild(m);
    }
    return m;
  }
  function closeMenu() { const m = S && S.root.querySelector('#g-menu'); if (m) m.classList.add('hidden'); }
  function showMenu(pos, title, items) {
    const m = menuEl();
    m.innerHTML = `<div class="g-menu-title">${esc(title)}</div>` + items.map(([k, t, tip]) => `<button data-mi="${k}" title="${esc(tip || '')}">${esc(t)}</button>`).join('');
    m.style.left = Math.round(pos.x + 8) + 'px';
    m.style.top = Math.round(pos.y + 8) + 'px';
    m.classList.remove('hidden');
    return m;
  }
  const HIDE = ['hide', 'Hide this node', 'take it and its lines off the drawing until "Show all hidden nodes" (this browser tab only)'];
  function menuItems(n) {
    if (isCode(n)) return [
      ['child', 'Add new child node…', 'a context, container or component this node owns, with its requirement and test files'],
      ['edge', 'Add new edge…', 'draw a line to another node: ownership or an interface to a part, a use case that needs it, an actor that uses it'],
      ['tests', 'Define tests…', 'the tests this part should pass, simplest first — optionally queue the test agent to build them'],
      ['runtests', 'Run its tests', 'queue a one-off run of this node\'s test groups'],
      HIDE];
    if (n.level === 'actor') return [
      ['edge', 'Add new edge…', 'draw a line to the part this actor uses (an interface that part provides)'],
      ['editactor', 'Edit actor…', 'its name and description'],
      HIDE];
    if (n.level === 'usecase') return [
      ['edge', 'Add new edge…', 'draw a line to a part this use case needs'],
      ['nameparts', 'Queue the usecase agent to name its parts', 'the agent reads the use case and lists every part its journey needs; the map tags each part and its owners'],
      ['edituc', 'Edit its text…', 'the markdown under the use case\'s frontmatter'],
      HIDE];
    if (n.level === 'project') return [
      ['ctx', '+ Context…', 'a new top-level area'],
      ['actor', '+ Actor…', 'a person or outside program at the edge of the map'],
      ['global', '+ Global object…', 'a business or technical requirement, or a use case']];
    return [HIDE];
  }
  function nodeMenu(n, pos) {
    if (S.ctx.myRole === 'viewer') {
      const m = showMenu(pos, n.name, [HIDE]);
      m.querySelector('[data-mi]').onclick = () => { closeMenu(); IterGraph.hide(n.id); };
      return;
    }
    const m = showMenu(pos, n.name, menuItems(n));
    m.querySelectorAll('[data-mi]').forEach((b) => {
      b.onclick = () => { closeMenu(); act(b.dataset.mi, n); };
    });
  }
  function act(k, n) {
    ({ child: () => openForm('child', n), edge: () => startDraw(n), tests: () => openForm('definetests', n), definetests: () => openForm('definetests', n),
      runtests: () => openForm('nodetests', n), nodetests: () => openForm('nodetests', n), hide: () => IterGraph.hide && IterGraph.hide(n.id),
      editactor: () => openForm('editactor', n), nameparts: () => openForm('nameparts', n), edituc: () => openForm('editglobal', n),
      ctx: () => openForm('ctx', null), actor: () => openForm('actor', null), global: () => openForm('global', null) })[k]();
  }
  function edgeMenu(id, pos) {
    if (S.ctx.myRole === 'viewer') return;
    const m = window.IterGraph.edgeInfo ? IterGraph.edgeInfo(id) : null;
    if (!m || !['contains', 'interface', 'usecase_touches'].includes(m.type)) return;
    if (m.type === 'usecase_touches' && String(m.target).startsWith('actor:')) return; // an actor takes part through the steps: edit them
    const title = m.type === 'contains' ? 'Ownership edge' : m.type === 'usecase_touches' ? 'Use-case edge' : String(m.source).startsWith('actor:') ? 'Actor edge' : 'Interface edge';
    const label = m.type === 'usecase_touches' ? 'Remove a part it needs…' : 'Remove edge…';
    const menu = showMenu(pos, title, [['remove', label, 'expire this edge; the reason is recorded in the commit']]);
    menu.querySelector('[data-mi]').onclick = () => { closeMenu(); openForm('removeedge', null, m); };
  }

  // ------------------------------------------------------------------ draw an edge
  let DRAW = null; // {from, move}
  function startDraw(n) {
    const cy = cyNow();
    if (!cy) return;
    cancelDraw();
    const src = cy.getElementById(n.id);
    if (src.empty()) return;
    const p = src.position();
    cy.add([{ group: 'nodes', data: { id: 'g-draw-cursor' }, position: { x: p.x + 40, y: p.y + 40 }, classes: 'g-draw-cursor' },
      { group: 'edges', data: { id: 'g-draw-edge', source: n.id, target: 'g-draw-cursor' }, classes: 'g-draw-edge' }]);
    // the dot under the pointer and the line to it take no clicks: the click
    // must land on the node under them (the edge's target)
    cy.style().selector('.g-draw-cursor').style({ width: 6, height: 6, 'background-color': '#4da3ff', label: '', events: 'no' })
      .selector('.g-draw-edge').style({ 'line-color': '#4da3ff', 'line-style': 'dashed', width: 2, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#4da3ff', 'curve-style': 'straight', opacity: 1, events: 'no' }).update();
    const move = (e) => { const c = cy.getElementById('g-draw-cursor'); if (c.nonempty()) c.position(e.position); };
    cy.on('mousemove', move);
    DRAW = { from: n, move };
    banner(`Drawing an edge from <b>${esc(n.name)}</b>: click the node it should reach (Esc cancels).`);
  }
  function cancelDraw() {
    const cy = cyNow();
    if (DRAW && cy) { cy.off('mousemove', DRAW.move); cy.remove('#g-draw-cursor, #g-draw-edge'); }
    DRAW = null;
    banner('');
  }
  function finishDraw(targetId) {
    const from = DRAW.from;
    cancelDraw();
    const to = S.byId.get(targetId);
    if (!to || to.id === from.id) { cancelDraw(); return; }
    const lv = (n) => (isCode(n) ? 'code' : n.level);
    const pair = lv(from) + '>' + lv(to);
    // an actor uses a part; a use case needs a part — either end may be drawn first
    if (pair === 'code>code') return openForm('edgeform', from, { to });
    if (pair === 'actor>code') return openForm('actoruses', from, { to });
    if (pair === 'code>actor') return openForm('actoruses', to, { to: from });
    if (pair === 'usecase>code') return openForm('ucneeds', from, { to });
    if (pair === 'code>usecase') return openForm('ucneeds', to, { to: from });
    banner('An edge joins two code nodes (ownership or an interface), an actor and the part it uses, or a use case and a part it needs.', 5000);
  }
  function banner(html, ms) {
    const b = S.root.querySelector('#g-banner');
    if (!b) return;
    b.innerHTML = html; b.classList.toggle('hidden', !html);
    if (ms) setTimeout(() => { if (b.innerHTML === html) b.classList.add('hidden'); }, ms);
  }

  // ------------------------------------------------------------------ node panel actions
  function decorate(d) {
    if (!d || d.kind !== 'node' || !d.node || !S || S.ctx.myRole === 'viewer') return;
    const n = d.node;
    // the panel carries the same actions as the node's menu (Hide has its own button there)
    const items = menuItems(n).filter(([k]) => k !== 'hide');
    if (!items.length) return;
    const div = document.createElement('div');
    div.className = 'actions';
    div.innerHTML = items.map(([k, t, tip]) => `<button class="plain" data-act="${k}" title="${esc(tip || '')}">${esc(t.replace(/…$/, ''))}</button>`).join('');
    const h2 = d.box.querySelector('h2');
    (h2 && h2.nextSibling) ? d.box.insertBefore(div, h2.nextSibling.nextSibling || null) : d.box.appendChild(div);
    div.querySelectorAll('[data-act]').forEach((b) => { b.onclick = () => act(b.dataset.act, n); });
  }

  // ------------------------------------------------------------------ forms
  async function full() {
    if (!S.full) S.full = await S.ctx.api('/api/projects/' + encodeURIComponent(S.ctx.project) + '/graph');
    return S.full;
  }
  const codeNodes = () => S.G.nodes.filter(isCode);
  const nodeOptions = (sel) => codeNodes().map((n) => `<option value="${esc(fileOf(n))}"${n.id === sel ? ' selected' : ''}>${esc(n.level)} · ${esc(n.name)} (${esc(n.id)})</option>`).join('');
  function dialog() {
    let d = S.root.querySelector('#g-editdlg');
    if (!d) { d = document.createElement('dialog'); d.id = 'g-editdlg'; d.className = 'g-editdlg'; S.root.appendChild(d); }
    return d;
  }
  const hint = (t) => `<span class="kv">${t}</span>`;
  const textRows = () => `
    <label>Description ${hint('one sentence, action first: “Reads …, checks … and hands … so that …” (a pure lookup: “Lists …; it runs nothing itself”)')}<input type="text" data-f="description" placeholder="Does X so that Y."></label>
    <label>Plain summary ${hint('one sentence for a reader with no technical background')}<input type="text" data-f="simple_description"></label>
    <label>Long description ${hint('what it does, how, what it takes and hands out, why it matters, one example — an agent fills this in if you leave it short')}<textarea data-f="long_description" rows="5"></textarea></label>`;
  const testsEditor = (title) => `
    <fieldset class="g-tests"><legend>${title} ${hint('optional — simplest first (e.g. “the container boots”), most complex last')}</legend>
      <ol data-tests></ol>
      <button class="plain" type="button" data-addtest>+ add a test</button>
      <label class="g-inline"><input type="checkbox" data-f="queue_agent"> queue the <b>test</b> agent to build and run these tests ${hint('(red is expected until the code exists; red groups become work items)')}</label>
    </fieldset>`;
  function wireTests(d) {
    const ol = d.querySelector('[data-tests]');
    if (!ol) return;
    const add = () => {
      const li = document.createElement('li');
      li.innerHTML = `<input type="text" data-tn placeholder="name, e.g. boots"><input type="text" data-td placeholder="what passing means">
        <button type="button" class="plain" data-up title="simpler">↑</button><button type="button" class="plain" data-rm title="remove">×</button>`;
      li.querySelector('[data-rm]').onclick = () => li.remove();
      li.querySelector('[data-up]').onclick = () => { if (li.previousElementSibling) ol.insertBefore(li, li.previousElementSibling); };
      ol.appendChild(li);
    };
    d.querySelector('[data-addtest]').onclick = () => add();
  }
  const readTests = (d) => [...d.querySelectorAll('[data-tests] li')].map((li) => ({ name: li.querySelector('[data-tn]').value.trim(), desc: li.querySelector('[data-td]').value.trim() })).filter((t) => t.name || t.desc);

  async function openForm(kind, node, extra) {
    const d = dialog();
    let title = '', body = '';
    if (kind === 'ctx' || kind === 'child') {
      title = kind === 'ctx' ? 'New context' : `New node owned by ${node.name}`;
      const lv = kind === 'ctx' ? 'context' : NEXT[node.level];
      body = `<label>Level ${hint('any level may own any level')}<select data-f="kind">${LEVELS.map((l) => `<option${l === lv ? ' selected' : ''}>${l}</option>`).join('')}</select></label>
        <label>Name ${hint('what a person would call it, 2–6 words')}<input type="text" data-f="name" required></label>
        ${textRows()}
        <label>Business requirements ${hint('optional — written to name.bizreq.iter.md')}<textarea data-f="bizreq" rows="3" placeholder="- B1. …"></textarea></label>
        <label>Technical requirements ${hint('optional — written to name.techreq.iter.md')}<textarea data-f="techreq" rows="3" placeholder="- T1. …"></textarea></label>
        ${testsEditor('Tests')}`;
    }
    if (kind === 'edgeform') {
      title = `Edge: ${node.name} → ${extra.to.name}`;
      body = `<label>Edge type<select data-f="etype"><option value="owns">${esc(node.name)} owns ${esc(extra.to.name)} (ownership)</option>
          <option value="uses">${esc(node.name)} uses an interface ${esc(extra.to.name)} provides (connection)</option></select></label>
        <div data-iface class="hidden">
          <label>What it does ${hint('2–5 plain words, shown on the edge')}<input type="text" data-f="ilabel" placeholder="Reads a balance"></label>
          <label>Interface name ${hint('kebab-case, one operation: service-operation')}<input type="text" data-f="iname" placeholder="ledger-read-balance"></label>
          <label>Kind<select data-f="ikind"><option value="request-reply">call and answer</option><option value="event">event</option><option value="dataset">shared dataset</option></select></label>
          <label>What it carries<input type="text" data-f="idesc"></label>
        </div>`;
    }
    if (kind === 'actor' || kind === 'editactor') {
      title = kind === 'actor' ? 'New actor' : `Edit actor ${node.name}`;
      body = `<p class="kv">A person or outside program at the edge of the map. Draw an edge from it to each part it uses.</p>
        <label>Name ${hint('who it is: Developer, Operator, Payment provider')}<input type="text" data-f="name" value="${esc(node ? node.name : '')}" required></label>
        <label>Description ${hint('what they do with the system, one sentence')}<input type="text" data-f="description" value="${esc(node ? node.description || '' : '')}"></label>`;
    }
    if (kind === 'actoruses') {
      title = `Edge: ${node.name} uses ${extra.to.name}`;
      // the interfaces this part already provides, by what they do
      const provided = new Map();
      S.G.edges.filter((e) => e.type === 'interface' && e.target === extra.to.id && e.interface).forEach((e) => provided.set(e.interface, e));
      body = `<p class="kv">${esc(node.name)} uses something ${esc(extra.to.name)} provides — a page, a command, an API. Name it, or pick one it already provides.</p>
        <label>Interface<select data-f="ipick"><option value="">a new interface…</option>${[...provided.values()].map((e) => `<option value="${esc(e.interface)}">${esc(e.label || e.interface)} (${esc(e.interface)})</option>`).join('')}</select></label>
        <div data-newiface>
          <label>What it is ${hint('2–5 plain words, shown on the edge')}<input type="text" data-f="ilabel" placeholder="Uses the web page"></label>
          <label>Interface id ${hint('kebab-case; blank = made from the words above')}<input type="text" data-f="iname" placeholder="webui-page"></label>
          <label>What it carries<input type="text" data-f="idesc"></label>
        </div>`;
    }
    if (kind === 'ucneeds') {
      title = `Edge: ${node.name} needs ${extra.to.name}`;
      body = `<p class="kv">Lists <b>${esc(extra.to.name)}</b> in the use case's <code>children.codenodes</code>. The map then tags it and every owner above it with the use case, and draws it under the use case's top-level part.</p>`;
    }
    if (kind === 'nameparts') {
      title = `Name the parts of ${node.name}`;
      body = `<p class="kv">Queues the <b>usecase</b> agent: it reads the use case and lists every part its journey needs, at the most specific level that is true. The map tags each part and its owners when the change lands.</p>`;
    }
    if (kind === 'definetests') {
      title = `Define tests for ${node.name}`;
      body = `<p class="kv">Written to the node's <code>test/…tests.iter.md</code> as a “Planned tests” list (it replaces the previous list). This is test-driven: define what passing means now; the test agent writes the scripts, they run red, and each red group becomes a work item to make it pass.</p>${testsEditor('Tests')}`;
    }
    if (kind === 'removeedge' && extra.type === 'usecase_touches') {
      const u = (S.G.usecases || []).find((x) => x.id === extra.source) || {};
      const named = (u.codenodes || []).map((id) => S.byId.get(id)).filter(Boolean);
      title = `Remove a part ${u.name || extra.source} needs`;
      body = (named.length ? `<label>Part ${hint('the parts its file lists; their owners drop with them')}<select data-f="ucpart">${named.map((n) => `<option value="${esc(fileOf(n))}">${esc(n.level)} · ${esc(n.name)}</option>`).join('')}</select></label>`
        : '<p class="kv">Its file lists no parts directly (they come from its flowmap steps): edit the use case to change them.</p>')
        + `<label>Why ${hint('required — recorded in the commit message')}<textarea data-f="reason" rows="3"></textarea></label>`;
    } else if (kind === 'removeedge') {
      const m = extra;
      const src = S.byId.get(m.source), tgt = S.byId.get(m.target);
      const ifaces = m.type === 'interface' ? [...new Set((m.members || []).map((e) => e.interface).filter(Boolean))] : [];
      title = m.type === 'contains' ? `Remove: ${src ? src.name : m.source} owns ${tgt ? tgt.name : m.target}` : `Remove interface edge ${src ? src.name : m.source} → ${tgt ? tgt.name : m.target}`;
      body = (ifaces.length > 1 ? `<label>Interface<select data-f="iface">${ifaces.map((i) => `<option>${esc(i)}</option>`).join('')}</select></label>`
        : ifaces.length ? `<p>Interface <code>${esc(ifaces[0])}</code></p><input type="hidden" data-f="iface" value="${esc(ifaces[0])}">` : '')
        + `<label>Why ${hint('required — recorded in the commit message')}<textarea data-f="reason" rows="3"></textarea></label>`;
    }
    if (kind === 'global') {
      title = 'New global object';
      body = `<label>Kind<select data-f="kind"><option value="bizreq">business requirement (bizreq)</option><option value="techreq">technical requirement (techreq)</option><option value="usecase">use case</option></select></label>
        <label>Name<input type="text" data-f="name" required></label><label>Description<input type="text" data-f="description"></label>
        <label>Text (markdown)<textarea data-f="body" rows="8" placeholder="# Title&#10;&#10;- R1. …"></textarea></label>
        <label>Parts it needs ${hint('use cases only, optional: the usecase agent is queued to name every part the journey needs; the map then tags each part and its owners, and draws the use case from its top-level parts down')}<select data-f="codenodes" multiple size="6">${nodeOptions()}</select></label>`;
    }
    if (kind === 'editglobal') {
      title = node ? `Edit ${node.name}` : 'Edit a global object';
      const g = await full();
      const globals = g.vertices.filter((v) => ['bizreq', 'techreq', 'usecase', 'interface'].includes(v.nodetype)).sort((a, b) => (a.path || '').localeCompare(b.path || ''));
      body = `<label>Object<select data-f="path">${globals.map((v) => `<option value="${esc(v.path)}">${esc(v.nodetype)} · ${esc(v.name)} — ${esc(v.path)}</option>`).join('')}</select></label>
        <label>Text (markdown, replaces everything under the frontmatter)<textarea data-f="body" rows="14"></textarea></label>`;
    }
    if (kind === 'connect') {
      title = 'Connect two parts';
      body = `<label>Uses (the caller / consumer)<select data-f="from">${nodeOptions()}</select></label><label>Provides (the service / producer)<select data-f="to">${nodeOptions()}</select></label>
        <label>Interface name<input type="text" data-f="iname" placeholder="ledger-read-balance" required></label>
        <label>What it does ${hint('2–5 plain words, shown on the edge')}<input type="text" data-f="ilabel" placeholder="Reads a balance"></label>
        <label>Kind<select data-f="ikind"><option value="request-reply">call and answer</option><option value="event">event</option><option value="dataset">shared dataset</option></select></label>
        <label>What it carries<input type="text" data-f="idesc"></label>`;
    }
    if (kind === 'tests' || kind === 'nodetests') {
      title = node ? `Run the tests of ${node.name}` : 'Run tests';
      const g = await full();
      const tvs = g.vertices.filter((v) => v.nodetype === 'tests' || v.nodetype === 'testgroup');
      let pick = tvs;
      if (node && node.vertex) { const mine = new Set(g.edges.filter((e) => e.from === node.vertex && (e.kind === 'tests' || e.kind === 'testgroups')).map((e) => e.to)); pick = tvs.filter((v) => mine.has(v.id)); }
      const groups = [];
      pick.forEach((v) => (v.groups || []).forEach((gr) => groups.push({ label: gr.label, path: v.path, result: (v.test && v.test.result) || gr.result || '' })));
      body = groups.length ? `<label>Test group<select data-f="group">${groups.map((x) => `<option value="${esc(x.label)}">${esc(x.label)} — ${esc(x.path)}${x.result ? ' (' + esc(x.result) + ')' : ''}</option>`).join('')}</select></label>`
        : `<p class="kv">No test groups ${node ? 'belong to this node' : 'are on the map'} yet${node ? ' — use Define tests first' : ''}.</p>`;
    }
    d.innerHTML = `<form method="dialog"><h3>${esc(title)}</h3>${body}<div class="g-msg kv"></div>
      <div class="g-btns"><button class="plain" value="cancel" formnovalidate>Cancel</button><button class="plain primary" value="ok" data-go>${kind === 'removeedge' ? 'Remove it' : 'Save'}</button></div></form>`;
    wireTests(d);
    if (kind === 'edgeform') { const sel = d.querySelector('[data-f=etype]'), box = d.querySelector('[data-iface]'); sel.onchange = () => box.classList.toggle('hidden', sel.value !== 'uses'); }
    if (kind === 'actoruses') { const sel = d.querySelector('[data-f=ipick]'), box = d.querySelector('[data-newiface]'); sel.onchange = () => box.classList.toggle('hidden', !!sel.value); }
    if (kind === 'editglobal') {
      const g = await full();
      const sel = d.querySelector('[data-f=path]'), ta = d.querySelector('[data-f=body]');
      const fill = () => { const v = g.vertices.find((x) => x.path === sel.value); ta.value = (v && v.body) || ''; };
      if (node && node.path) { const want = '{topdir}/' + node.path; if ([...sel.options].some((o) => o.value === want)) sel.value = want; }
      sel.onchange = fill; fill();
    }
    d.querySelector('[data-go]').onclick = async (ev) => {
      ev.preventDefault();
      const f = (k) => { const el = d.querySelector(`[data-f=${k}]`); if (!el) return ''; if (el.type === 'checkbox') return el.checked; return el.multiple ? [...el.selectedOptions].map((o) => o.value) : el.value.trim(); };
      const msg = d.querySelector('.g-msg');
      let op;
      if (kind === 'ctx' || kind === 'child') {
        op = { op: 'new_node', kind: f('kind'), name: f('name'), description: f('description'), simple_description: f('simple_description'),
          long_description: f('long_description'), bizreq: f('bizreq'), techreq: f('techreq'), tests: readTests(d), queue_agent: !!f('queue_agent') };
        if (kind === 'child') op.parent = fileOf(node);
        if (!op.name) { msg.textContent = 'a name is required'; return; }
        if (!op.description) { msg.textContent = 'a one-sentence description is required (say what it does)'; return; }
        if (op.queue_agent && !op.tests.length) { msg.textContent = 'add at least one test for the test agent to build'; return; }
      }
      if (kind === 'edgeform') {
        if (f('etype') === 'owns') op = { op: 'link_child', parent: fileOf(node), child: fileOf(extra.to) };
        else op = { op: 'connect', from: fileOf(node), to: fileOf(extra.to), interface: { name: f('iname'), label: f('ilabel'), kind: f('ikind'), description: f('idesc') } };
        if (op.op === 'connect' && !op.interface.name) { msg.textContent = 'name the interface'; return; }
      }
      if (kind === 'definetests') {
        op = { op: 'define_tests', node: fileOf(node), tests: readTests(d), queue_agent: !!f('queue_agent') };
        if (!op.tests.length) { msg.textContent = 'add at least one test'; return; }
      }
      if (kind === 'removeedge') {
        const m = extra, reason = f('reason');
        if (!reason) { msg.textContent = 'say why — it is recorded in the commit'; return; }
        const src = S.byId.get(m.source), tgt = S.byId.get(m.target);
        if (m.type === 'usecase_touches') {
          if (!f('ucpart')) { msg.textContent = 'no listed part to remove'; return; }
          op = { op: 'usecase_unneed', usecase: fileOf(src), node: f('ucpart'), reason };
        } else if (String(m.source).startsWith('actor:')) {
          op = { op: 'actor_unuse', actor: m.source, interface: f('iface'), reason };
        } else {
          op = m.type === 'contains' ? { op: 'unlink_child', parent: fileOf(src), child: fileOf(tgt), reason }
            : { op: 'disconnect', from: fileOf(src), to: fileOf(tgt), interface: { name: f('iface') }, reason };
        }
      }
      if (kind === 'actor' || kind === 'editactor') {
        op = kind === 'actor' ? { op: 'new_actor', name: f('name'), description: f('description') }
          : { op: 'edit_actor', actor: node.id, name: f('name'), description: f('description') };
        if (!f('name')) { msg.textContent = 'a name is required'; return; }
      }
      if (kind === 'actoruses') {
        const picked = f('ipick');
        const iname = picked || f('iname') || f('ilabel').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
        if (!iname) { msg.textContent = 'say what it uses, or pick an interface'; return; }
        op = { op: 'actor_uses', actor: node.id, to: fileOf(extra.to), interface: { name: iname, label: picked ? '' : f('ilabel'), description: f('idesc') } };
      }
      if (kind === 'ucneeds') op = { op: 'usecase_needs', usecase: fileOf(node), node: fileOf(extra.to) };
      if (kind === 'nameparts') op = { op: 'name_parts', usecase: fileOf(node) };
      if (kind === 'global') op = { op: 'new_global', kind: f('kind'), name: f('name'), description: f('description'), body: f('body'), codenodes: f('kind') === 'usecase' ? f('codenodes') : [] };
      if (kind === 'editglobal') op = { op: 'edit_body', path: f('path'), body: d.querySelector('[data-f=body]').value };
      if (kind === 'connect') op = { op: 'connect', from: f('from'), to: f('to'), interface: { name: f('iname'), label: f('ilabel'), kind: f('ikind'), description: f('idesc') } };
      if (kind === 'tests' || kind === 'nodetests') { op = { op: 'run_tests', group: f('group') }; if (!op.group) { msg.textContent = 'no test group to run'; return; } }
      if (op.op === 'new_global' && !op.name) { msg.textContent = 'a name is required'; return; }
      try {
        const r = await S.ctx.api('/api/projects/' + encodeURIComponent(S.ctx.project) + '/graph/edits', { method: 'POST', body: JSON.stringify(op) });
        if (r.state === 'pending') {
          msg.innerHTML = `Accepted — <b>waiting to sync to an engine</b>. The first engine serving <b>${esc(S.ctx.project)}</b> picks it up on its next heartbeat, writes the files, commits and pushes them; the map redraws here when it lands.`;
        } else {
          msg.innerHTML = `queued as test item <b>…${esc((r.id || '').slice(-12))}</b> in the work queue.`;
        }
        d.querySelector('[data-go]').remove();
        refreshSync();
      } catch (e) { msg.textContent = 'refused: ' + e.message; }
    };
    if (!d.open) d.showModal();
  }

  // ------------------------------------------------------------------ sync indicator
  let polling = null, lastApplied = null;
  async function refreshSync() {
    if (!S) return;
    const p = S.ctx.project;
    let rows = [];
    try { rows = await S.ctx.api('/api/projects/' + encodeURIComponent(p) + '/datasync'); } catch (e) { return; }
    if (!S || S.ctx.project !== p) return;
    const open = rows.filter((r) => r.state === 'pending' || r.state === 'claimed');
    const failed = rows.filter((r) => r.state === 'failed').slice(0, 5);
    const applied = rows.filter((r) => r.state === 'applied');
    const newest = applied.length ? applied[0].id : '';
    if (lastApplied !== null && newest && newest !== lastApplied) { lastApplied = newest; S.ctx.reload(); return; }
    lastApplied = newest;
    const el = S.root.querySelector('#g-sync');
    if (el) {
      const bits = [];
      if (open.length) bits.push(`<span class="g-pend">⟳ ${open.length} waiting to sync</span>`);
      if (failed.length) bits.push(`<span class="g-fail">✗ ${failed.length} failed</span>`);
      el.innerHTML = bits.join(' ');
      el.title = rows.slice(0, 12).map((r) => `${r.state.padEnd(8)} ${r.summary}${r.engine ? ' · ' + r.engine : ''}${r.error ? ' — ' + r.error : ''}${r.commit ? ' · ' + r.commit.slice(0, 8) : ''}`).join('\n') || 'no graph edits yet';
      el.onclick = () => showSync(rows);
    }
    clearTimeout(polling);
    if (open.length) polling = setTimeout(refreshSync, 5000);
  }
  function showSync(rows) {
    const d = dialog();
    const tr = (r) => `<tr><td>${esc(r.state)}</td><td>${esc(r.summary)}</td><td>${esc(r.engine || '')}</td><td>${esc((r.commit || '').slice(0, 8))}</td><td>${esc(r.error || r.note || '')}</td></tr>`;
    d.innerHTML = `<form method="dialog"><h3>Graph edits — ${esc(S.ctx.project)}</h3>
      <p class="kv">An edit waits until an engine serving the project picks it up (usually within one heartbeat); an edit touching a folder a running item has locked waits for that item.</p>
      <div style="max-height:50vh;overflow:auto"><table class="g-synctable"><tr><th>state</th><th>change</th><th>engine</th><th>commit</th><th>note</th></tr>${rows.slice(0, 40).map(tr).join('') || '<tr><td colspan=5>none yet</td></tr>'}</table></div>
      <div class="g-btns"><button class="plain" value="close">Close</button></div></form>`;
    if (!d.open) d.showModal();
  }
}());
