/* iter5 webui — the GraphRAG tab (from iter4, 2026-09-29).
 *
 * Manage a project's GraphRAG index: upload documents (pdf, docx, html, md,
 * txt…), see the *.iter.md node files the change sweep indexed, watch the
 * Summary agent's progress, search (full chunks back, with the map around
 * node hits), set the docs directory, and create the scheduled change sweep.
 * Self-contained like intro.js: index.html loads rag.css + rag.js and calls
 *   IterRag.mount(el, ctx)   once, with an empty <div> the tab owns
 *   IterRag.show(ctx)        whenever the tab becomes visible or the project changes
 *   IterRag.hide()           when the tab is left (stops polling)
 * ctx = {api(path, opts) -> Promise<json>, project, isAdmin, canWrite, openGraph(q), openQueue(), openItem(id)}.
 * Every class is prefixed rag-; colours come from index.html's :root tokens.
 */
(function () {
  "use strict";
  const esc = s => String(s == null ? "" : s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const enc = encodeURIComponent;
  const store = {
    get(k) { try { return JSON.parse(localStorage.getItem(k) || "null"); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (e) { /* private window */ } }
  };
  const NODETYPES = ["project", "code", "test", "bizreq", "techreq", "philosophy", "usecase", "actor"];

  let el = null, ctx = null, timer = null;
  let S = { status: null, docs: [], filter: "", kind: store.get("iter5.rag_kind") || "file", open: null, openDoc: null, results: null, uploads: [] };

  const P = () => "/api/projects/" + enc(ctx.project);
  const ago = iso => {
    if (!iso) return "";
    const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
    return s < 90 ? Math.round(s) + "s ago" : s < 5400 ? Math.round(s / 60) + "m ago" : s < 129600 ? Math.round(s / 3600) + "h ago" : Math.round(s / 86400) + "d ago";
  };
  const count = (rows, pred) => (rows || []).filter(pred).reduce((a, r) => a + (r.n || 0), 0);
  // a heading path that starts with the document's own title ("iter guide > …",
  // "GraphRAG index (code) > …") repeats what the hit's title already says
  const trimHeading = (heading, title) => {
    const parts = String(heading || "").split(" > ");
    const t = String(title || "").trim().toLowerCase();
    if (parts.length > 1 && t && parts[0].trim().toLowerCase().startsWith(t)) parts.shift();
    return parts.join(" > ");
  };

  function mount(target, c) {
    el = target; ctx = c;
    el.innerHTML = `
      <div class="rag-wrap">
        <section class="rag-head" id="rag-status"><span class="rag-dim">loading GraphRAG…</span></section>
        <div class="rag-cols">
          <div class="rag-left">
            <section class="rag-card">
              <h3>Search</h3>
              <form id="rag-sform" class="rag-sform">
                <input id="rag-q" type="search" placeholder="Ask in plain words — e.g. how does the engine claim a work item?" autocomplete="off">
                <button type="submit">Search</button>
                <button type="button" class="ghost" id="rag-all" title="Every document in the index, one per row, with its summary and where its file is — not ranked">Show all documents</button>
              </form>
              <div class="rag-sopts">
                <label>in <select id="rag-skind"><option value="">everything</option><option value="file">uploaded documents</option><option value="node">node files</option><option value="guide">the iter guide</option></select></label>
                <label>node type <select id="rag-snt"><option value="">any</option>${NODETYPES.map(t => `<option>${t}</option>`).join("")}</select></label>
                <label>match by <select id="rag-smode"><option value="hybrid">keywords + meaning</option><option value="vector">meaning only</option><option value="keyword">keywords only</option></select></label>
                <label>vectors <select id="rag-svec"><option value="both">raw + summary</option><option value="raw">raw text</option><option value="summary">summary</option></select></label>
                <label>top <select id="rag-sk"><option>5</option><option selected>8</option><option>15</option><option>30</option></select></label>
              </div>
              <div id="rag-results"></div>
            </section>
            <section class="rag-card">
              <div class="rag-row"><h3>Documents</h3>
                <div class="rag-seg" id="rag-kinds"><button data-k="file">Uploaded</button><button data-k="node">Node files</button></div>
                <input id="rag-filter" type="search" placeholder="filter by title or path" class="rag-grow">
              </div>
              <div id="rag-docs"></div>
            </section>
          </div>
          <div class="rag-right">
            <section class="rag-card" id="rag-detail"><span class="rag-dim">Select a document to see its summary, chapters and chunks.</span></section>
            <section class="rag-card">
              <h3>Upload documents</h3>
              <div id="rag-drop" class="rag-drop">
                <b>Drop documents here</b> or <label class="rag-link">choose files<input id="rag-file" type="file" multiple hidden
                  accept=".pdf,.docx,.pptx,.md,.markdown,.txt,.html,.htm,.csv,.json,.yaml,.yml,.rst,.adoc,.log,.xml,.toml"></label>
                <div class="rag-dim">pdf (scans are OCR'd) · docx · pptx · html · md · txt and other text. An engine extracts, chunks and embeds it within seconds, the Summary agent adds summaries, and the original is written to the docs directory.</div>
                <label class="rag-dim"><input type="checkbox" id="rag-store" checked> store the original in the repo</label>
                <div id="rag-uploads"></div>
              </div>
            </section>
            <section class="rag-card">
              <h3>Settings</h3>
              <label class="rag-field">Docs directory <span class="rag-dim">(where the engine stores uploaded originals)</span>
                <div class="rag-row"><input id="rag-docsdir" class="rag-grow" placeholder="{topdir}/docs/"><button id="rag-docsdir-save" class="ghost">Save</button></div></label>
              <label class="rag-field">Also index from the checkout <span class="rag-dim">(where the files live; one glob per line, e.g. docs/**/*.md)</span>
                <textarea id="rag-globs" rows="2" class="rag-grow" placeholder="core/notes/**/*.md"></textarea></label>
              <label class="rag-field">Node file types to index <span class="rag-dim">(blank = all; e.g. code, bizreq, techreq)</span>
                <div class="rag-row"><input id="rag-ntypes" class="rag-grow" placeholder="all node types"><button id="rag-index-save" class="ghost">Save</button></div></label>
              <label class="rag-dim rag-check"><input type="checkbox" id="rag-gitignore"> add the docs directory to the project's .gitignore
                (originals are written there but never committed)</label>
              <div id="rag-sched"></div>
              <div id="rag-mcp" class="rag-dim"></div>
            </section>
          </div>
        </div>
      </div>`;
    const q = s => el.querySelector(s);
    q("#rag-sform").onsubmit = ev => { ev.preventDefault(); search(); };
    q("#rag-all").onclick = () => showAll();
    q("#rag-filter").oninput = ev => { S.filter = ev.target.value.toLowerCase(); renderDocs(); };
    el.querySelectorAll("#rag-kinds button").forEach(b => b.onclick = () => { S.kind = b.dataset.k; store.set("iter5.rag_kind", S.kind); renderDocs(); });
    q("#rag-file").onchange = ev => { upload([...ev.target.files]); ev.target.value = ""; };
    const drop = q("#rag-drop");
    drop.ondragover = ev => { ev.preventDefault(); drop.classList.add("over"); };
    drop.ondragleave = () => drop.classList.remove("over");
    drop.ondrop = ev => { ev.preventDefault(); drop.classList.remove("over"); upload([...ev.dataTransfer.files]); };
    q("#rag-docsdir-save").onclick = saveDocsDir;
    q("#rag-gitignore").onchange = saveGitignore;
    q("#rag-index-save").onclick = saveIndexing;
    show(c);
  }

  function show(c) {
    const changed = !ctx || ctx.project !== c.project;
    ctx = c;
    if (changed) { S.open = null; S.openDoc = null; S.openSig = null; S.results = null; S.docs = []; el.querySelector("#rag-results").innerHTML = ""; renderDetail(); }
    refresh(true);
    clearInterval(timer);
    timer = setInterval(() => { if (!document.hidden) refresh(false); }, 5000);
  }
  function hide() { clearInterval(timer); timer = null; }

  async function refresh(full) {
    if (!ctx || !ctx.project) { el.querySelector("#rag-status").innerHTML = "<span class='rag-dim'>pick a project</span>"; return; }
    const p = ctx.project;
    try {
      const [st, docs] = await Promise.all([ctx.api(P() + "/rag"), ctx.api(P() + "/rag/docs")]);
      if (p !== ctx.project) return;
      S.status = st; S.docs = docs;
      renderStatus(); renderDocs(); if (full) renderSettings();
      if (S.open) {
        const d = await ctx.api(P() + "/rag/docs/" + enc(S.open)).catch(() => null);
        // redraw only when the document really changed: a redraw on every poll
        // closed the chunk the reader had just opened (2026-09-30)
        const sig = d && JSON.stringify(d);
        if (p === ctx.project && S.open && d && sig !== S.openSig) { S.openSig = sig; S.openDoc = d; renderDetail(); }
      }
    } catch (e) {
      el.querySelector("#rag-status").innerHTML = `<span class="rag-bad">could not load GraphRAG: ${esc(e.message)}</span>`;
    }
  }

  function renderStatus() {
    const st = S.status;
    const files = count(st.docs, r => r.kind === "file"), nodes = count(st.docs, r => r.kind === "node");
    const ready = count(st.docs, r => r.state === "ready"), total = files + nodes;
    const done = count(st.chunks, r => r.state === "done"), chunks = count(st.chunks, () => true);
    const failed = count(st.chunks, r => r.state === "failed");
    const eng = st.engines || [];
    const live = eng.filter(e => !e.hold);
    const engLine = live.length
      ? `<span class="rag-ok">●</span> Summary agent: engine <b>${esc(live.map(e => e.name).join(", "))}</b>${live[0].account ? " on " + esc(live[0].account) : ""}`
      : eng.length
        ? `<span class="rag-warn">●</span> engine ${esc(eng[0].name)} is <b>suspended</b> (${esc(eng[0].hold)}) — summaries wait`
        : `<span class="rag-bad">●</span> <b>no live engine serves ${esc(ctx.project)}</b> — documents are searchable by raw text; summaries wait for an engine`;
    const model = st.model || {};
    const modelLine = model.loaded || model.stamp ? `<span title="${esc(model.stamp || "")}">${esc(model.model)}</span>` : model.error ? `<span class="rag-bad">model: ${esc(model.error)}</span>` : `<span class="rag-bad">embedding model not found</span>`;
    const vi = st.vector_index || {};
    const viLine = (vi.indexes || []).length >= 2 ? "vector index ✓" : vi.note ? `<span title="${esc(vi.note)}">exact search (no vector index)</span>` : "exact search";
    const mm = st.model_mismatch ? ` · <span class="rag-bad" title="these chunks were embedded by different model weights than the ones that embed search questions here; re-sync or re-upload them">${st.model_mismatch} chunks from another model</span>` : "";
    el.querySelector("#rag-status").innerHTML = `
      <div class="rag-stats">
        <div><b>${total}</b><span>documents</span><i>${files} uploaded · ${nodes} node files · ${ready} ready</i></div>
        <div><b>${chunks}</b><span>chunks</span><i>${done} summarised${failed ? ` · <span class="rag-bad">${failed} failed</span>` : ""}</i></div>
        <div><b>${st.waiting || 0}</b><span>summaries waiting</span><i>for the Summary agent</i></div>
        <div class="rag-bar" title="${done}/${chunks} chunks summarised"><div style="width:${chunks ? Math.round(100 * done / chunks) : 0}%"></div></div>
      </div>
      <div class="rag-line">${engLine} <span class="rag-dim">· ${modelLine} · ${viLine}</span>${mm}</div>`;
  }

  function stateBadge(d) {
    const n = d.chunks || 0, s = d.summarized || 0;
    if (d.state === "ready") return `<span class="rag-badge ok">ready</span>`;
    if (d.state === "rollup") return `<span class="rag-badge q" title="every chunk is summarised; the chapter and document summaries are next">rollup</span>`;
    if (d.state === "summarizing") return `<span class="rag-badge warn" title="searchable by raw text now">summarising ${s}/${n}</span>`;
    if (d.state === "queued") return `<span class="rag-badge dim" title="waiting for an engine to extract, chunk and embed it">waiting for an engine</span>`;
    if (d.state === "ingesting") return `<span class="rag-badge warn" title="an engine is extracting, chunking and embedding it">ingesting</span>`;
    if (d.state === "failed") return `<span class="rag-badge bad" title="${esc(d.error || "")}">failed</span>`;
    return `<span class="rag-badge">${esc(d.state || "?")}</span>`;
  }
  function storeBadge(d) {
    const s = d.store && d.store.state;
    if (!s) return "";
    const t = { pending: "waiting for an engine to write it", storing: "an engine is writing it", stored: "committed" + (d.store.commit ? " " + d.store.commit.slice(0, 8) : ""),
      written: "written to the docs directory, not committed (git-ignored, or the checkout is not a git repository root)", failed: "store failed: " + (d.store.error || "") }[s] || s;
    return `<span class="rag-badge ${s === "stored" || s === "written" ? "ok" : s === "failed" ? "bad" : "dim"}" title="${esc(t)}">${s === "stored" ? "in repo" : s === "written" ? "in docs dir" : s}</span>`;
  }

  function renderDocs() {
    el.querySelectorAll("#rag-kinds button").forEach(b => b.classList.toggle("on", b.dataset.k === S.kind));
    // the upload card stands on its own (right column): shown to writers whatever list is open
    el.querySelector("#rag-drop").hidden = !ctx.canWrite;
    const rows = S.docs.filter(d => d.kind === S.kind && (!S.filter || (d.title + " " + d.path).toLowerCase().includes(S.filter)));
    const box = el.querySelector("#rag-docs");
    if (!rows.length) {
      box.innerHTML = S.kind === "node"
        ? `<div class="rag-empty">No node files indexed yet. The <b>GraphRAG change sweep</b> (Settings → create the schedule) runs <code>iter rag sync</code> on an engine,
           which indexes every <code>*.iter.md</code> file of the map and re-indexes the ones that change. From a checkout you can also run <code>iter rag sync</code> by hand.</div>`
        : `<div class="rag-empty">${S.filter ? "Nothing matches." : "No uploaded documents yet."}</div>`;
      return;
    }
    box.innerHTML = `<table class="rag-table"><thead><tr><th>Document</th><th>State</th><th class="rag-num">Chunks</th><th>Updated</th></tr></thead><tbody>${rows.map(d => `
      <tr data-id="${esc(d.id)}" class="${S.open === d.id ? "sel" : ""}">
        <td><div class="rag-title">${d.kind === "node" ? `<span class="rag-nt">${esc(d.nodetype || "node")}</span> ` : ""}${esc(d.title)}</div><div class="rag-path">${esc(d.path)}</div></td>
        <td>${stateBadge(d)} ${storeBadge(d)}${d.linked ? ` <span class="rag-badge q" title="map nodes that name this document">◉ ${d.linked}</span>` : ""}</td>
        <td class="rag-num">${d.chunks || 0}</td>
        <td class="rag-dim">${esc(ago(d.updated))}</td></tr>`).join("")}</tbody></table>`;
    box.querySelectorAll("tr[data-id]").forEach(tr => tr.onclick = () => openDoc(tr.dataset.id));
  }

  async function openDoc(id) {
    S.open = id; S.openDoc = null; renderDocs();
    el.querySelector("#rag-detail").innerHTML = "<span class='rag-dim'>loading…</span>";
    try { S.openDoc = await ctx.api(P() + "/rag/docs/" + enc(id)); S.openSig = JSON.stringify(S.openDoc); } catch (e) { el.querySelector("#rag-detail").innerHTML = `<span class="rag-bad">${esc(e.message)}</span>`; return; }
    renderDetail();
    if (window.innerWidth < 900) el.querySelector("#rag-detail").scrollIntoView({ behavior: "smooth" });
  }

  function renderDetail() {
    const box = el.querySelector("#rag-detail");
    // what the reader has open, and where they scrolled, survive a redraw
    const openChunks = new Set([...box.querySelectorAll("details.rag-chunk[open]")].map(x => x.dataset.i));
    const scroll = box.scrollTop;
    const typed = (box.querySelector("#rag-linknode") || {}).value || "";
    const d = S.openDoc;
    if (!d) { box.innerHTML = "<span class='rag-dim'>Select a document to see its summary, chapters and chunks.</span>"; return; }
    const chunks = d.chunk_list || [];
    const chapters = (d.chapters || []).filter(c => c.title || c.summary || (d.chapters || []).length > 1);
    box.innerHTML = `
      <div class="rag-row"><h3 class="rag-grow">${esc(d.title)}</h3><button class="ghost rag-x" id="rag-close" title="close">✕</button></div>
      <div class="rag-path">${esc(d.path)}</div>
      <div class="rag-meta">${stateBadge(Object.assign({ summarized: chunks.filter(c => c.sum_state === "done").length }, d))} ${storeBadge(d)}
        <span class="rag-dim">${esc(d.format || "")} · ${(d.chars || 0).toLocaleString()} chars · ${d.chunks} chunks · ${(d.chapters || []).length} chapters${d.summary_model ? " · summaries by " + esc(d.summary_model) : ""}</span></div>
      ${d.kind === "node" && d.node_id ? `<div class="rag-row"><button class="ghost rag-sm" id="rag-ongraph">◉ show on the Project graph</button></div>` : ""}
      ${d.kind === "file" ? `<h4>Map nodes it describes</h4>
        <div class="rag-nb">${(d.linked_nodes || []).map(n => `<span class="rag-chip"><button class="rag-link" data-graph="${esc(n.path)}">${esc(n.name || n.path)}</button> <i>${esc(n.nodetype)}</i>${ctx.canWrite ? ` <button class="rag-x2" data-unlink="${esc(n.path)}" title="unlink">✕</button>` : ""}</span>`).join("") || "<span class='rag-dim'>none yet — link the nodes this document describes, so searches that hit either bring the other</span>"}</div>
        ${ctx.canWrite ? `<div class="rag-row rag-linkrow"><input id="rag-linknode" class="rag-grow" list="rag-nodelist" placeholder="link to a map node — type its name or path"><datalist id="rag-nodelist"></datalist><button class="ghost rag-sm" id="rag-linkbtn">Link</button></div>
          <div id="rag-linknote" class="rag-dim"></div>` : ""}` : ""}
      <h4>Document summary</h4>
      <div class="rag-summary">${d.summary ? esc(d.summary) : `<span class="rag-dim">${d.state === "ready" ? "none" : "the Summary agent writes it once every chunk is summarised"}</span>`}</div>
      ${d.error ? `<div class="rag-bad">${esc(d.error)}</div>` : ""}
      ${chapters.length ? `<h4>Chapters</h4>${chapters.map(c => `<div class="rag-chap"><b>${esc(c.title || "(untitled)")}</b> <span class="rag-dim">${c.chunks} chunk${c.chunks === 1 ? "" : "s"}</span><div>${c.summary ? esc(c.summary) : "<span class='rag-dim'>no summary yet</span>"}</div></div>`).join("")}` : ""}
      <h4>Chunks</h4>
      ${chunks.map(c => `<details class="rag-chunk" data-i="${c.idx}"${openChunks.has(String(c.idx)) ? " open" : ""}><summary><span class="rag-badge ${c.sum_state === "done" ? "ok" : c.sum_state === "failed" ? "bad" : "dim"}">${esc(c.sum_state)}</span>
          <b>#${c.idx + 1}</b> ${esc(c.heading || "")} <span class="rag-dim">${c.chars} chars</span>
          <div class="rag-csum">${c.summary ? esc(c.summary) : c.sum_error ? `<span class="rag-bad">${esc(c.sum_error)}</span>` : ""}</div></summary>
          <pre class="rag-text">${esc(c.text)}</pre></details>`).join("")}
      ${ctx.canWrite ? `<div class="rag-actions">
        <button class="ghost rag-sm" id="rag-resum">Re-summarise</button>
        <button class="ghost rag-sm rag-danger" id="rag-del">Remove from GraphRAG…</button></div>` : ""}`;
    box.scrollTop = scroll;
    const ln = box.querySelector("#rag-linknode");
    if (ln && typed) ln.value = typed;
    box.querySelector("#rag-close").onclick = () => { S.open = null; S.openDoc = null; renderDetail(); renderDocs(); };
    box.querySelectorAll("[data-graph]").forEach(b => b.onclick = () => ctx.openGraph(b.dataset.graph));
    box.querySelectorAll("[data-unlink]").forEach(b => b.onclick = () => linkDoc(d, b.dataset.unlink, true));
    const lb = box.querySelector("#rag-linkbtn");
    if (lb) {
      fillNodeList(box.querySelector("#rag-nodelist"));
      lb.onclick = () => {
        const v = box.querySelector("#rag-linknode").value.trim();
        const hit = (S.nodes || []).find(n => n.path === v || n.label === v);
        linkDoc(d, hit ? hit.path : v, false);
      };
    }
    const og = box.querySelector("#rag-ongraph");
    if (og) og.onclick = () => ctx.openGraph(d.path);
    const rs = box.querySelector("#rag-resum");
    if (rs) rs.onclick = async () => {
      if (!confirm(`Throw away the ${chunks.length} chunk summaries of "${d.title}" and have the Summary agent write them again?`)) return;
      await ctx.api(P() + "/rag/docs/" + enc(d.id) + "/resummarize", { method: "POST" }).catch(e => alert(e.message));
      refresh(false);
    };
    const del = box.querySelector("#rag-del");
    if (del) del.onclick = async () => {
      let rm = false;
      if (d.kind === "file") {
        rm = confirm(`Remove "${d.title}" from GraphRAG.\n\nAlso delete the stored original ${d.path} from the repo (an engine commits the removal)?\n\nOK = remove from GraphRAG AND the repo · Cancel = decide below`);
        if (!rm && !confirm(`Remove "${d.title}" from GraphRAG only (the file stays in the repo)?`)) return;
      } else if (!confirm(`Remove the node file "${d.title}" from GraphRAG? (The next change sweep indexes it again while the file exists.)`)) return;
      await ctx.api(P() + "/rag/docs/" + enc(d.id) + (rm ? "?remove_file=true" : ""), { method: "DELETE" }).catch(e => alert(e.message));
      S.open = null; S.openDoc = null; renderDetail(); refresh(false);
    };
  }

  function renderSettings() {
    const st = S.status;
    const dd = el.querySelector("#rag-docsdir");
    if (document.activeElement !== dd) dd.value = (st.settings || {}).docs_dir || "{topdir}/docs/";
    dd.disabled = el.querySelector("#rag-docsdir-save").hidden = !ctx.canWrite;
    const set = st.settings || {};
    const gl = el.querySelector("#rag-globs"), nt = el.querySelector("#rag-ntypes");
    if (document.activeElement !== gl) gl.value = (set.repo_globs || []).join("\n");
    if (document.activeElement !== nt) nt.value = (set.node_types || []).join(", ");
    gl.disabled = nt.disabled = el.querySelector("#rag-index-save").hidden = !ctx.canWrite;
    const gi = el.querySelector("#rag-gitignore");
    gi.checked = !!(st.settings || {}).docs_gitignore;
    gi.disabled = !ctx.canWrite;
    const sc = st.schedule;
    const box = el.querySelector("#rag-sched");
    if (sc) {
      const every = sc.sched && sc.sched.every_min ? (sc.sched.every_min % 60 ? sc.sched.every_min + " min" : sc.sched.every_min / 60 + " h") : "?";
      box.innerHTML = `<h4>Change sweep</h4><div>Scheduled every <b>${esc(every)}</b>: <code>${esc(sc.exec_shell)}</code>
        <button class="ghost rag-sm" id="rag-sched-open">open the schedule</button></div>`;
      box.querySelector("#rag-sched-open").onclick = () => ctx.openItem(sc.id);
    } else {
      box.innerHTML = `<h4>Change sweep</h4>
        <div class="rag-dim">A scheduled work item that runs <code>iter rag sync</code> on an engine: every node file of the map whose text changed is re-indexed (unchanged chunks keep their summaries), and deleted ones are dropped.</div>
        ${ctx.canWrite ? `<div class="rag-row"><select id="rag-every"><option value="30">every 30 min</option><option value="60" selected>every hour</option><option value="240">every 4 h</option><option value="1440">daily</option></select>
          <button id="rag-sched-make">Create a scheduled RAG change sweep</button></div>` : ""}`;
      const mk = box.querySelector("#rag-sched-make");
      if (mk) mk.onclick = async () => {
        mk.disabled = true; mk.textContent = "creating…";
        try {
          const r = await ctx.api(P() + "/rag/schedule", { method: "POST", body: JSON.stringify({ every_min: +box.querySelector("#rag-every").value }) });
          mk.textContent = r.created ? "created" : "already scheduled";
          refresh(true);
        } catch (e) { mk.disabled = false; mk.textContent = "Create a scheduled RAG change sweep"; alert(e.message); }
      };
    }
    el.querySelector("#rag-mcp").innerHTML = `<h4>For agents (MCP)</h4>Stateless MCP at <code>${esc(location.origin)}/mcp</code>
      (HTTP, <code>Authorization: Bearer &lt;token&gt;</code>, optional <code>X-Iter-Project</code>). Tool <code>rag_search</code> returns full chunks with the map around them.`;
  }

  async function saveDocsDir() {
    const v = el.querySelector("#rag-docsdir").value.trim();
    try { const r = await ctx.api(P() + "/rag/settings", { method: "PUT", body: JSON.stringify({ docs_dir: v }) }); el.querySelector("#rag-docsdir").value = r.docs_dir; }
    catch (e) { alert(e.message); }
  }

  async function saveIndexing() {
    const globs = el.querySelector("#rag-globs").value.split("\n").map(x => x.trim()).filter(Boolean);
    const types = el.querySelector("#rag-ntypes").value.split(/[,\s]+/).map(x => x.trim()).filter(Boolean);
    try {
      await ctx.api(P() + "/rag/settings", { method: "PUT", body: JSON.stringify({ repo_globs: globs, node_types: types }) });
      toast("Saved — the next change sweep (or map change) indexes them");
      refresh(true);
    } catch (e) { alert(e.message); }
  }

  async function saveGitignore() {
    const v = el.querySelector("#rag-gitignore").checked;
    try { await ctx.api(P() + "/rag/settings", { method: "PUT", body: JSON.stringify({ docs_gitignore: v }) }); refresh(true); }
    catch (e) { alert(e.message); el.querySelector("#rag-gitignore").checked = !v; }
  }

  // the map's nodes, for the link box (loaded once per project)
  async function fillNodeList(dl) {
    if (!S.nodes || S.nodesFor !== ctx.project) {
      const g = await ctx.api(P() + "/graph").catch(() => ({ nodes: [] }));
      S.nodes = (g.nodes || g.vertices || []).filter(v => v.path && !v.deleted && !["test", "tests", "testgroup", "agentmem"].includes(v.nodetype))
        .map(v => ({ path: v.path, label: `${v.name || v.path} (${v.nodetype})` }));
      S.nodesFor = ctx.project;
    }
    if (dl) dl.innerHTML = S.nodes.map(n => `<option value="${esc(n.label)}">${esc(n.path)}</option>`).join("");
  }

  async function linkDoc(d, node, unlink) {
    if (!node) return;
    const note = el.querySelector("#rag-linknote");
    try {
      await ctx.api(P() + "/rag/docs/" + enc(d.id) + "/links", { method: "POST", body: JSON.stringify({ node, unlink }) });
      if (note) note.textContent = `${unlink ? "Unlink" : "Link"} queued: the next engine writes it into ${node} and the map picks it up on its next sync.`;
    } catch (e) { alert(e.message); }
  }

  function toast(msg) {
    const t = document.createElement("div");
    t.className = "rag-toast"; t.textContent = msg;
    document.body.appendChild(t);
    setTimeout(() => t.remove(), 2200);
  }

  function b64(file) {
    return new Promise((res, rej) => {
      const r = new FileReader();
      r.onload = () => res(String(r.result).split(",")[1] || "");
      r.onerror = () => rej(r.error);
      r.readAsDataURL(file);
    });
  }

  async function upload(files) {
    if (!files.length) return;
    const keep = el.querySelector("#rag-store").checked;
    const box = el.querySelector("#rag-uploads");
    for (const f of files) {
      const row = document.createElement("div");
      row.className = "rag-up";
      row.innerHTML = `<b>${esc(f.name)}</b> <span class="rag-dim">${(f.size / 1024).toFixed(0)} KB — reading…</span>`;
      box.prepend(row);
      const note = row.querySelector("span");
      if (f.size > 25 * 1024 * 1024) { note.innerHTML = `<span class="rag-bad">too large (limit 25 MB)</span>`; continue; }
      if (/\.iter\.md$/i.test(f.name)) { note.innerHTML = `<span class="rag-bad">*.iter.md files are map nodes: the change sweep indexes them from the checkout</span>`; continue; }
      try {
        const content = await b64(f);
        note.textContent = "uploading…";
        const d = await ctx.api(P() + "/rag/docs", { method: "POST", body: JSON.stringify({ filename: f.name, content_b64: content, store: keep }) });
        note.innerHTML = `<span class="rag-ok">received</span> — an engine extracts, chunks and embeds it next${keep ? `; the original goes to <code>${esc(d.path)}</code>` : ""}`;
        S.kind = "file";
      } catch (e) {
        note.innerHTML = `<span class="rag-bad">${esc(e.message)}</span>`;
      }
    }
    refresh(false);
  }

  // "Show all documents": one card per document (not per chunk) with its
  // summary and where its file lives — nothing is matched, so nothing is scored.
  // Honours the "in" and "node type" filters; the user guide is included.
  async function showAll() {
    const box = el.querySelector("#rag-results");
    const kind = el.querySelector("#rag-skind").value, nt = el.querySelector("#rag-snt").value;
    box.innerHTML = "<span class='rag-dim'>loading every document…</span>";
    let docs, guide = [];
    try {
      docs = await ctx.api(P() + "/rag/docs" + (kind && kind !== "guide" ? "?kind=" + enc(kind) : ""));
      if ((!kind || kind === "guide") && !nt) guide = await ctx.api("/api/projects/_iter/rag/docs").catch(() => []);
    } catch (e) { box.innerHTML = `<span class="rag-bad">${esc(e.message)}</span>`; return; }
    if (kind === "guide") docs = [];
    if (nt) docs = docs.filter(d => d.nodetype === nt);
    const order = { file: 0, node: 1, guide: 2 };
    const all = docs.concat(guide).sort((a, b) => (order[a.kind] ?? 3) - (order[b.kind] ?? 3) || String(a.nodetype || "").localeCompare(String(b.nodetype || "")) || String(a.title).localeCompare(String(b.title)));
    S.results = null;
    const where = d => {
      if (d.kind === "guide") return `<div class="rag-loc"><span class="rag-dot live"></span><b>built into the data server</b> <span class="rag-dim">${esc(d.path)}</span></div>`;
      const locs = d.locations || [];
      if (!locs.length) return `<div class="rag-loc"><span class="rag-dot"></span><span class="rag-dim">no engine serves this project · ${esc(d.path)}</span></div>`;
      return locs.map(l => `<div class="rag-loc" title="${l.live ? "engine is live" : "engine is offline"}"><span class="rag-dot ${l.live ? "live" : ""}"></span><b>${esc(l.engine)}</b> <code>${esc(l.path)}</code></div>`).join("");
    };
    const n = all.length, counts = ["file", "node", "guide"].map(k => [k, all.filter(d => d.kind === k).length]).filter(([, c]) => c);
    box.innerHTML = `<div class="rag-dim rag-rmeta">${n} document${n === 1 ? "" : "s"} (${counts.map(([k, c]) => `${c} ${{ file: "uploaded", node: "node files", guide: "guide" }[k]}`).join(" · ")}) · every document in the index, not ranked
        <button class="ghost rag-sm" id="rag-all-close">close</button></div>
      <div class="rag-alldocs">${all.map(d => `<article class="rag-doccard">
        <header>${d.kind === "node" ? `<span class="rag-nt">${esc(d.nodetype || "node")}</span>` : d.kind === "guide" ? `<span class="rag-nt">guide</span>` : `<span class="rag-nt">${esc(d.format || "file")}</span>`}
          <button class="rag-link" data-open="${esc(d.kind === "guide" ? "" : d.id)}" ${d.kind === "guide" ? "disabled" : ""}>${esc(d.title)}</button>
          ${stateBadge(d)} <span class="rag-dim rag-small">${d.chunks || 0} chunks · ${(d.chapters || []).length} chapters · updated ${esc(ago(d.updated))}</span></header>
        <div class="rag-docsum">${d.summary ? esc(d.summary) : `<span class="rag-dim">${d.state === "ready" ? "no summary" : "the Summary agent has not written this document's summary yet"}</span>`}</div>
        ${where(d)}
      </article>`).join("")}</div>`;
    box.querySelector("#rag-all-close").onclick = () => { box.innerHTML = ""; };
    box.querySelectorAll("[data-open]").forEach(b => b.onclick = () => { if (!b.dataset.open) return; const d = S.docs.find(x => x.id === b.dataset.open); if (d) S.kind = d.kind; openDoc(b.dataset.open); });
  }

  async function search() {
    const q = el.querySelector("#rag-q").value.trim();
    const box = el.querySelector("#rag-results");
    if (!q) return;
    const kind = el.querySelector("#rag-skind").value, nt = el.querySelector("#rag-snt").value;
    const body = { query: q, k: +el.querySelector("#rag-sk").value, vectors: el.querySelector("#rag-svec").value,
      mode: el.querySelector("#rag-smode").value, kinds: kind ? [kind] : [], nodetypes: nt ? [nt] : [], graph: true, docs: true };
    box.innerHTML = "<span class='rag-dim'>searching…</span>";
    let r;
    try { r = await ctx.api(P() + "/rag/search", { method: "POST", body: JSON.stringify(body) }); }
    catch (e) { box.innerHTML = `<span class="rag-bad">${esc(e.message)}</span>`; return; }
    const res = r.results || [];
    const cand = r.candidates || {};
    box.innerHTML = `<div class="rag-dim rag-rmeta">${res.length} result${res.length === 1 ? "" : "s"} · ${r.ms} ms · ${esc(r.mode)} (${cand.keyword || 0} keyword, ${cand.raw || 0} raw, ${cand.summary || 0} summary candidates) · ${esc(r.method)}</div>`
      + (r.documents && r.documents.length ? `<div class="rag-docsrank">Closest documents: ${r.documents.slice(0, 5).map(d => `<button class="rag-chip" data-open="${esc(d.id)}" title="${esc(d.summary || "")}">${esc(d.title)} <i>${d.score.toFixed(2)}</i></button>`).join("")}</div>` : "")
      + res.map((h, i) => {
        const d = h.document || {};
        const g = h.graph || {};
        const nb = g.neighbours || [];
        const sim = h.similarity == null ? "—" : h.similarity.toFixed(2);
        const rk = h.ranks || {};
        return `<article class="rag-hit">
          <header><span class="rag-score" title="cosine similarity to the question (raw ${h.score_raw == null ? "—" : h.score_raw.toFixed(3)} · summary ${h.score_summary == null ? "—" : h.score_summary.toFixed(3)}); fused rank score ${h.score.toFixed(4)}">${sim}</span>
            ${(h.matched || []).map(m => `<span class="rag-badge ${m === "keyword" ? "q" : "dim"}" title="rank ${rk[m]} by ${m}">${esc(m)} #${rk[m]}</span>`).join(" ")}
            ${d.kind === "node" ? `<span class="rag-nt">${esc(d.nodetype || "node")}</span>` : d.kind === "guide" ? `<span class="rag-nt" title="the built-in iter user guide">guide</span>` : ""}
            <button class="rag-link" data-open="${esc(h.doc)}">${esc(d.title)}</button>
            <span class="rag-dim">${esc(trimHeading(h.heading, d.title) || h.chapter_title || "")}</span></header>
          <div class="rag-path">${esc(d.path)} · chunk ${h.idx + 1}</div>
          ${h.summary ? `<div class="rag-csum">${esc(h.summary)}</div>` : ""}
          <details ${i < 2 ? "open" : ""}><summary class="rag-dim">full chunk</summary><pre class="rag-text">${esc(h.text)}</pre></details>
          ${h.chapter_summary ? `<div class="rag-ctx"><b>Chapter${h.chapter_title ? " · " + esc(h.chapter_title) : ""}:</b> ${esc(h.chapter_summary)}</div>` : ""}
          ${d.summary ? `<div class="rag-ctx"><b>Document:</b> ${esc(d.summary)}</div>` : ""}
          ${nb.length ? `<div class="rag-nb"><b>In the map:</b> ${nb.slice(0, 14).map(n => `<button class="rag-chip" data-graph="${esc(n.path || n.id)}" title="${esc(n.dir === "in" ? "links to this node" : "this node links to it")} (${esc(n.kind)})">${n.dir === "in" ? "←" : "→"} ${esc(n.name || n.id)} <i>${esc(n.nodetype || "")}</i></button>`).join("")}</div>` : ""}
          ${(g.documents || []).length ? `<div class="rag-nb"><b>Its documents:</b> ${g.documents.map(x => `<button class="rag-chip" data-open="${esc(x.id || "")}" title="${esc(x.summary || x.path)}">▤ ${esc(x.title || x.path)}</button>`).join("")}</div>` : ""}
          ${(g.linked_nodes || []).length ? `<div class="rag-nb"><b>Describes:</b> ${g.linked_nodes.map(n => `<button class="rag-chip" data-graph="${esc(n.path)}">◉ ${esc(n.name || n.path)} <i>${esc(n.nodetype || "")}</i></button>`).join("")}</div>` : ""}
        </article>`;
      }).join("");
    box.querySelectorAll("[data-open]").forEach(b => b.onclick = () => { const d = S.docs.find(x => x.id === b.dataset.open); if (d) { S.kind = d.kind; } openDoc(b.dataset.open); });
    box.querySelectorAll("[data-graph]").forEach(b => b.onclick = () => ctx.openGraph(b.dataset.graph));
  }

  window.IterRag = { mount, show, hide };
})();
