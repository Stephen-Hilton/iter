/* iter4 webui — the Intro tab (rebuilt 2026-09-30).
 *
 * One story, told at three levels of detail. A 3-stop slider (Summary ·
 * Business · Technical) never changes the slide or its picture: stop 1 is the
 * high-level summary, stop 2 lays business callouts over it, stop 3 adds the
 * technical overlay (API routes, config keys, code names). Two pages sit
 * beside the story, each behind its own header button: the Glossary and the
 * new-project wizard ("Start a new project").
 *
 * Self-contained: index.html loads intro.css + intro.js and calls
 *   IterIntro.mount(el, ctx)   once, with an empty <div> the tab owns
 *   IterIntro.show(ctx)        whenever the tab becomes visible again
 * ctx = {api(path, opts) -> Promise<json>, project, isAdmin,
 *        openGraph(nodeQuery), openQueue(), reloadProjects()}.
 * Every class is prefixed intro-; colours come from index.html's :root tokens.
 * Keys: ← → move between slides, ↑ ↓ change the level of detail. A checkbox under
 * the slide (remembered per browser) resets the detail to Summary on every slide change.
 */
(function () {
  "use strict";

  /* ------------------------------------------------------------ helpers */
  const esc = s => String(s == null ? "" : s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const store = {
    get(k) { try { return JSON.parse(localStorage.getItem(k) || "null"); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (e) { /* private window: fine */ } }
  };
  const POS_KEY = "iter_intro_v2";

  let copies = []; // copy-button payloads for the current render
  function code(caption, text) {
    const i = copies.push(text) - 1;
    return `<div class="intro-code"><div class="intro-cap"><span>${esc(caption)}</span>` +
      `<button class="intro-btn intro-ghost intro-sm" data-copy="${i}">Copy</button></div><pre>${esc(text)}</pre></div>`;
  }
  const gbtn = (query, label) => `<button class="intro-btn intro-ghost intro-sm" data-graph="${esc(query)}" title="Open the Project graph at ${esc(query)}">&#9673; ${esc(label || query)}</button>`;
  const qbtn = label => `<button class="intro-btn intro-ghost" data-queue="1">${esc(label || "Open the work queue")}</button>`;
  const chip = (cls, t) => `<span class="intro-chip ${cls || ""}">${t}</span>`;
  const card = (cls, tag, title, body) => `<div class="intro-card ${cls}">${tag ? `<span class="intro-tag">${tag}</span>` : ""}<h4>${title}</h4>${body}</div>`;

  function toast(msg) {
    const t = document.createElement("div");
    t.className = "intro-toast"; t.textContent = msg;
    document.body.appendChild(t);
    setTimeout(() => t.remove(), 1400);
  }
  async function copyText(text) {
    try { await navigator.clipboard.writeText(text); toast("Copied"); return; } catch (e) { /* fall back */ }
    const ta = document.createElement("textarea");
    ta.value = text; ta.style.position = "fixed"; ta.style.opacity = "0";
    document.body.appendChild(ta); ta.select();
    try { document.execCommand("copy"); toast("Copied"); } catch (e) { toast("Copy failed: select the text by hand"); }
    ta.remove();
  }

  /* ------------------------------------------------------------ levels of detail */
  const LEVELS = [
    { n: 1, label: "Summary", hint: "the idea in one picture" },
    { n: 2, label: "Business", hint: "adds what it means for the business" },
    { n: 3, label: "Technical", hint: "adds how it is built" }
  ];
  // content shown from level n up; business callouts and technical notes look different
  const at = (n, html) => (S.lv >= n ? html : "");
  const biz = html => at(2, `<div class="intro-ov intro-ov-biz"><span class="intro-ovtag">business</span>${html}</div>`);
  const tech = html => at(3, `<div class="intro-ov intro-ov-tech"><span class="intro-ovtag">technical</span>${html}</div>`);
  // one line that grows with the level: [summary, business addition, technical addition]
  function grow(parts) {
    let h = `<span>${parts[0]}</span>`;
    if (S.lv >= 2 && parts[1]) h += ` <span class="intro-g-biz">${parts[1]}</span>`;
    if (S.lv >= 3 && parts[2]) h += `<span class="intro-g-tech">${parts[2]}</span>`;
    return h;
  }

  /* ------------------------------------------------------------ visuals */

  // the cover: the whole system in one picture, with numbered steps pinned on it
  // (how a project gets going, in order; hover or tap a number for its text).
  // Summary draws the parts; Business adds what each part is responsible for;
  // Technical adds the node files, binaries and wire protocols.
  const COVER_PINS = [
    // [step, x, y, title, one line] in the svg's 940x560 viewBox
    [1, 104, 42, "you", "Set requirements, describe the work, and define or refine the tests that say what \u201cdone\u201d means."],
    [2, 30, 150, "your repo", "Create your repository, drop in iter_engine, and connect it to iter_data."],
    [3, 822, 20, "iter_data", "Start refining your requirements and tests, and optionally use the Project graph to lay out your project's high-level design."],
    [4, 420, 290, "iter_engine", "Set it up and turn it on: it starts processing work items."],
    [5, 30, 470, "agents", "Agents work off each other's results to create plans, code, tests and more, using the MCP server in iter_data."],
    [6, 652, 296, "kept in sync", "Work is synced between one central iter_data, any number of installed iter_engines, and any number of Claude accounts."]
  ];
  function coverSvg() {
    const b = S.lv >= 2, t = S.lv >= 3;
    const tabs = (x, w, y0, items, cls) => items.map((l, i) => `<g class="intro-c-tab ${cls}">
      <rect x="${x}" y="${y0 + i * 24}" width="${w}" height="21" rx="5"/><text x="${x + w / 2}" y="${y0 + i * 24 + 14.5}" text-anchor="middle">${l}</text></g>`).join("");
    // the repository tree: [indent, label, technical note]
    const tree = [[0, "Your_Repo/", "+ main.iter.md"], [1, "src/"], [2, "data/"], [3, "container_db/", "+ db.code.iter.md"], [4, "src/"], [4, "test/", "*.tests.iter.md"],
      [2, "app/"], [3, "container_applogic/"], [4, "src/"], [4, "test/"], [1, ".iter/", "config.json"]];
    const treeSvg = tree.map(([d, l, note], i) => {
      const y = 182 + i * 20, x = 52 + d * 16;
      const label = d ? `<tspan class="intro-c-hook">&#8627; </tspan>${l}` : l;
      const n = t && note ? `<text x="${d === 1 ? 140 : 226}" y="${y}" class="intro-c-note">${note}</text>` : "";
      return `<text x="${x}" y="${y}" class="intro-c-tree ${d ? "" : "intro-c-root"}">${label}</text>${n}`;
    }).join("");
    const agent = (cx, name, stack) => {
      const cy = 488, r = 33;
      const back = stack ? `<circle class="intro-c-agentb" cx="${cx - 14}" cy="${cy - 12}" r="${r}"/><circle class="intro-c-agentb" cx="${cx - 7}" cy="${cy - 6}" r="${r}"/>` : "";
      const top = stack ? cy - 12 - r : cy - r;
      return `<line class="intro-c-write" x1="${cx}" y1="${top - 3}" x2="${cx}" y2="416" marker-end="url(#intro-cah)"/>
        <line class="intro-c-bus" x1="${cx}" y1="545" x2="${cx}" y2="${cy + r + 4}" marker-end="url(#intro-cah)"/>
        <g class="intro-c-agent">${back}<circle cx="${cx}" cy="${cy}" r="${r}"/>
        <text x="${cx}" y="${cy + 1}" text-anchor="middle" class="intro-c-aname">${name}</text>
        <text x="${cx}" y="${cy + 15}" text-anchor="middle" class="intro-c-asub">${t ? "claude -p" : "agent"}</text></g>`;
    };
    const webEnd = b ? 540 : 636;
    return `<svg class="intro-c-svg" viewBox="0 0 940 560" role="img" aria-label="Your repository holds your code; iter_engine runs inside it and starts Test, Code, Deploy and Plan agents that write to the repository; the engine talks to iter_data over the API and the agents reach iter_data over MCP; you steer iter_data from the web page">
      <defs>
        <marker id="intro-cah" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" class="intro-v-ahead"/></marker>
        <marker id="intro-cah-b" viewBox="0 0 10 10" refX="7" refY="5" markerWidth="4.5" markerHeight="4.5" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" class="intro-c-ahead-b"/></marker>
      </defs>

      <g class="intro-c-you"><circle cx="70" cy="70" r="28"/><text x="70" y="75" text-anchor="middle" class="intro-c-youl">you</text>
        <text x="70" y="118" text-anchor="middle" class="intro-v-cap">file work · answer</text><text x="70" y="132" text-anchor="middle" class="intro-v-cap">set limits</text></g>
      <line class="intro-c-web" x1="102" y1="70" x2="${webEnd}" y2="70" marker-end="url(#intro-cah)"/>
      <text x="${(102 + webEnd) / 2}" y="62" text-anchor="middle" class="intro-c-wire">web page</text>
      ${t ? `<text x="${(102 + webEnd) / 2}" y="88" text-anchor="middle" class="intro-c-note">browser · login token</text>` : ""}

      ${b ? tabs(544, 102, 26, ["work queue", "lock mgmt", "schedule", "agent defs"], "intro-c-tab-d") + tabs(816, 104, 26, ["account mgmt", "engine mgmt", "arbitration", "MCP / APIs"], "intro-c-tab-d") : ""}
      <g class="intro-c-data"><rect x="640" y="20" width="180" height="104" rx="14"/>
        <text x="730" y="54" text-anchor="middle" class="intro-c-title">iter_data</text>
        <text x="730" y="76" text-anchor="middle" class="intro-c-sub">control panel</text>
        <text x="730" y="98" text-anchor="middle" class="intro-v-cap">${t ? "rust binary + ArangoDB" : "one container"}</text></g>

      <g class="intro-c-repo"><rect x="30" y="150" width="560" height="262" rx="10"/></g>
      ${treeSvg}

      <path class="intro-c-api" d="M586,340 C650,340 700,260 700,130" marker-start="url(#intro-cah-b)" marker-end="url(#intro-cah-b)"/>
      <text x="708" y="222" class="intro-c-wire intro-c-wireb">API</text>
      <text x="708" y="238" class="intro-v-cap">${t ? "" : "work &amp; results"}</text>
      ${t ? `<text x="708" y="238" class="intro-c-note">HTTPS JSON</text><text x="708" y="252" class="intro-c-note">engine token</text>` : ""}

      ${b ? tabs(316, 108, 296, ["agent mgmt", "repo mgmt", "account mgmt", "verification"], "intro-c-tab-e") : ""}
      <g class="intro-c-eng"><rect x="420" y="290" width="160" height="106" rx="12"/>
        <text x="500" y="326" text-anchor="middle" class="intro-c-title intro-c-title-s">iter_engine</text>
        <text x="500" y="348" text-anchor="middle" class="intro-c-sub">execution</text>
        <text x="500" y="370" text-anchor="middle" class="intro-v-cap">${t ? "rust binary · 5 s tick" : "beside your code"}</text></g>

      <path class="intro-c-bus" d="M560,396 L560,545 L90,545"/>
      <text x="566" y="438" class="intro-v-cap">${t ? "starts sessions" : "starts agents"}</text>
      ${agent(90, "Test", true)}${agent(220, "Code", true)}${agent(350, "Deploy", false)}${agent(470, "Plan", false)}
      <text x="160" y="436" text-anchor="middle" class="intro-v-cap">${t ? "edit files in the checkout" : "change the code"}</text>

      <path class="intro-c-mcp" d="M560,545 C740,545 808,380 808,130" marker-end="url(#intro-cah)"/>
      <text x="760" y="352" text-anchor="end" class="intro-c-wire">MCP</text>
      <text x="760" y="368" text-anchor="end" class="intro-v-cap">${t ? "POST /mcp: search, file work, ask" : "agents' tools"}</text>
    </svg>`;
  }
  const coverPins = () => COVER_PINS.map(([n, x, y, title, line]) => {
    const side = x > 600 ? "intro-tip-l" : x < 200 ? "intro-tip-r" : "", up = y > 320 ? "intro-tip-up" : "";
    return `<span class="intro-pin" tabindex="0" data-pin="${n}" style="left:${(x / 940 * 100).toFixed(2)}%;top:${(y / 560 * 100).toFixed(2)}%" aria-label="${n}. ${esc(title)}: ${esc(line)}">${n}` +
      `<span class="intro-tip ${side} ${up}" role="tooltip"><b>${n}. ${esc(title)}</b>${esc(line)}</span></span>`;
  }).join("");
  const coverStory = () => `<ol class="intro-pinlist">${COVER_PINS.map(([n, , , title, line]) =>
    `<li data-pin="${n}"><span class="intro-pinn">${n}</span><span><b>${title}</b> <span class="intro-g-biz">${line}</span></span></li>`).join("")}</ol>`;

  // the work loop as a circle: six steps, new work re-enters at "File"
  const LOOP = [
    { t: "File", d: ["Work is described: a first big request, or ongoing work as it comes up.", "Anyone can file, and so can agents: one big request is broken into many small items.", "POST /api/projects/{p}/workitems · MCP workitem_create · iter add. Children inherit priority and use-case tag; a repeat (same check: + container: tags) merges into the open item."] },
    { t: "Prioritize", d: ["A number from 0 to 99 decides the order; lower goes first.", "0–9 now · 10–39 one number per use case · 40–49 people's requests · 50+ maintenance. Related work shares one number, so it moves together.", "priority 0–99, blockedby (deep: waits for the blocker and all it created); cycles refused at write time, with the path named."] },
    { t: "Pick up", d: ["A free engine takes the most important item that can run now.", "Items that must wait say why in the queue: a reserved folder, a budget, another item.", "Engine tick every 5 s: versioned claim queued→in-progress, one lock row per lockdir, caps (maxagents ladder, agent max, daily budget)."] },
    { t: "Build", d: ["A Claude Code agent does the work in its copy of the repository.", "Several agents build at once, each inside the folders it reserved.", "claude -p turns: prework → request → postwork → agent memory → self-check. Every session has the iter MCP tools (search, file work, ask)."] },
    { t: "Check", d: ["A checker confirms the work matches the request.", "Incomplete work goes back once with the gaps listed; anything unclear goes to a person.", "Close gate: deterministic evidence (turn cap, commits, children, notes) + a verifier model → complete | bounce | question."] },
    { t: "Save", d: ["The change is saved and the item closes; follow-up work loops back to File.", "Every step is recorded on the item: request, replies, checks, cost.", "Scoped git commit (lockdirs + commit_extra_paths) and push; locks released; spend row written."] }
  ];
  function loopSvg() {
    const cx = 170, cy = 170, r = 118, n = LOOP.length;
    const pt = i => { const a = -Math.PI / 2 + i * 2 * Math.PI / n; return [cx + r * Math.cos(a), cy + r * Math.sin(a)]; };
    const arcs = LOOP.map((_, i) => {
      const a0 = -Math.PI / 2 + i * 2 * Math.PI / n + 0.26, a1 = -Math.PI / 2 + (i + 1) * 2 * Math.PI / n - 0.26;
      const [x0, y0] = [cx + r * Math.cos(a0), cy + r * Math.sin(a0)], [x1, y1] = [cx + r * Math.cos(a1), cy + r * Math.sin(a1)];
      return `<path class="intro-v-arc" d="M${x0.toFixed(1)},${y0.toFixed(1)} A${r},${r} 0 0 1 ${x1.toFixed(1)},${y1.toFixed(1)}" marker-end="url(#intro-ah2)"/>`;
    }).join("");
    const nodes = LOOP.map((s, i) => {
      const [x, y] = pt(i);
      return `<g class="intro-v-step intro-v-s${i}" data-step="${i}"><circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="27"/>
        <text x="${x.toFixed(1)}" y="${(y - 3).toFixed(1)}" text-anchor="middle" class="intro-v-num">${i + 1}</text>
        <text x="${x.toFixed(1)}" y="${(y + 11).toFixed(1)}" text-anchor="middle" class="intro-v-lbl">${s.t}</text></g>`;
    }).join("");
    return `<svg class="intro-v-loop" viewBox="0 0 340 340" role="img" aria-label="the work loop: file, prioritize, pick up, build, check, save, and back to file">
      <defs><marker id="intro-ah2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto"><path d="M0,0 L10,5 L0,10 z" class="intro-v-ahead"/></marker></defs>
      ${arcs}${nodes}
      <text x="${cx}" y="${cy - 6}" text-anchor="middle" class="intro-v-center">the work loop</text>
      <text x="${cx}" y="${cy + 12}" text-anchor="middle" class="intro-v-cap">every engine runs it,</text>
      <text x="${cx}" y="${cy + 26}" text-anchor="middle" class="intro-v-cap">many at once</text>
    </svg>`;
  }
  const loopLegend = () => `<ol class="intro-loopl">${LOOP.map((s, i) => `<li data-step="${i}"><b>${i + 1} · ${s.t}</b> ${grow(s.d.map((x, k) => k === 2 ? `<code>${esc(x)}</code>` : x))}</li>`).join("")}</ol>`;

  // a repository tree with folder reservations
  function lockTree() {
    const p = t => S.lv >= 3 ? `<code>{topdir}/${t}</code>` : `<b>${t}</b>`;
    return `<div class="intro-tree">
      <div>${S.lv >= 3 ? "<code>{topdir}/</code>" : "your repository/"}</div>
      <div class="intro-ind1">${p("api/")} ${chip("intro-ok", "reserved: Add refund endpoint")}</div>
      <div class="intro-ind2">${p("api/payments/")} ${chip("intro-w", "waits: Fix rounding (inside api/)")}</div>
      <div class="intro-ind1">${p("web/checkout/")} ${chip("intro-ok", "reserved: New checkout page")}</div>
      <div class="intro-ind1">${p("docs/")} ${chip("intro-d", "free")}</div>
    </div>`;
  }
  // a small dependency chain: B and C wait for A and everything A created
  function depChain() {
    return `<div class="intro-deps">
      <div class="intro-dep intro-dep-done">A · Design the API<small>+ the 3 items it filed</small></div>
      <span class="intro-deparr">&rarr;</span>
      <div class="intro-dep intro-dep-run">B · Build the API<small>waits for A and all of A's items</small></div>
      <span class="intro-deparr">&rarr;</span>
      <div class="intro-dep">C · Build the screen<small>waits for B</small></div>
    </div>`;
  }
  function priorityBands() {
    const band = (from, to, label, cls) => `<div class="intro-band ${cls}" style="flex:${to - from + 1}"><b>${from}–${to}</b><span>${label}</span></div>`;
    return `<div class="intro-bands">${band(0, 9, "do now", "intro-band-now")}${band(10, 39, "one per use case", "intro-band-uc")}${band(40, 49, "people", "intro-band-p")}${band(50, 99, "maintenance", "intro-band-m")}</div>
      <div class="intro-small intro-dim intro-center">lower number goes first</div>`;
  }

  // the close gate: one entrance, three exits
  function gateDiagram() {
    const t = S.lv >= 3;
    return `<div class="intro-gate">
      <div class="intro-gin"><b>The agent says "done"</b><span>${t ? "session result + final message + git evidence + notes" : "and writes what it delivered"}</span></div>
      <div class="intro-garr">&rarr;</div>
      <div class="intro-gbox">
        <div class="intro-gstage"><span class="intro-gn">1</span><b>Check the facts</b><span>${t ? "turn cap · open reviews · requires_children · requires_commit" : "did it finish, commit, file what it had to?"}</span></div>
        <div class="intro-gstage"><span class="intro-gn">2</span><b>Second opinion</b><span>${t ? "verifier model (closegate.verify, default haiku), read-only; JSON verdict" : "another AI compares the claim with the request"}</span></div>
      </div>
      <div class="intro-garr">&rarr;</div>
      <div class="intro-gouts">
        <div class="intro-gout intro-gout-ok"><b>&#10003; Complete</b><span>${t ? "close complete; commit + push" : "saved and closed"}</span></div>
        <div class="intro-gout intro-gout-back"><b>&#8634; Back once</b><span>${t ? "bounce under max_bounces (1): gaps go into the next prompt" : "with the gaps listed"}</span></div>
        <div class="intro-gout intro-gout-q"><b>? Ask a person</b><span>${t ? "question widget: continue | accept" : "when it is still unclear"}</span></div>
      </div>
    </div>`;
  }

  // usage: several accounts, each with a switch and a stop mark
  function ladderBars() {
    const row = (name, used, sw, st, note) => `<div class="intro-barrow"><b>${name}</b>
      <div class="intro-track-bar"><div class="intro-fill" style="width:${used}%"></div>
      <div class="intro-mark intro-sw" style="left:${sw}%"></div><div class="intro-mark intro-st" style="left:${st}%"></div></div>
      <span class="intro-dim">${note}</span></div>`;
    return `<div class="intro-bars">
      ${row("Account 1", 83, 80, 99, "past switch → next")}
      ${row("Account 2", 41, 80, 99, "in use now")}
      ${row("Account 3", 12, 80, 99, "waiting")}
      <div class="intro-small intro-dim"><span style="color:var(--warn)">&#9646;</span> switch % &nbsp; <span style="color:var(--bad)">&#9646;</span> stop % &nbsp; bar = the higher of 5-hour and 7-day usage</div>
    </div>`;
  }
  function controlPanel() {
    return `<div class="intro-panel">
      <div class="intro-pcell"><div class="intro-switch"><span class="intro-sw-on">Running</span><span>Stopped</span></div>
        <b>Run or stop a project</b><span>stopping lets running work finish first</span></div>
      <div class="intro-pcell"><div class="intro-inbox"><span>?</span><em>3</em></div>
        <b>The question inbox</b><span>agents and the checker ask; your answer sends the item back in line</span></div>
      <div class="intro-pcell"><div class="intro-gauge"><div style="width:62%"></div></div><div class="intro-small">$31 of $50 today</div>
        <b>A daily budget</b><span>blank = no limit · 0 = spend nothing</span></div>
    </div>`;
  }

  // where it runs: the looks, the brains, the brawn
  function archDiagram() {
    const t = S.lv >= 3;
    const eng = (n, agents) => `<div class="intro-node intro-eng"><span class="intro-role">${t ? "iter_engine" : "engine"} · machine ${n}</span>
         <h4>${t ? "Engine0" + n : "Engine " + n}</h4>
         <p>${t ? "Polls every 5 s, claims items, runs <code>claude -p</code> in its checkout, commits, pushes." : "Runs agents in its copy of the repository."}</p>
         <div class="intro-agents">${agents}</div></div>`;
    return `<div class="intro-arch">
      <div class="intro-node intro-web"><span class="intro-role">the looks</span><h4>Web page</h4>
        <p>${t ? "One page (<code>webui/</code>), plain JavaScript, built into the iter_data binary." : "Where people watch the work, answer questions and set limits."}</p></div>
      <div class="intro-wire"><span class="intro-arr">&#8646;</span>${t ? "HTTPS JSON<br>+ bearer token" : "reads &amp;<br>steers"}</div>
      <div class="intro-node intro-data"><span class="intro-role">the brains</span><h4>Data server</h4>
        <p>${t ? "axum HTTP API + MCP. The only thing that touches the database; the lock authority." : "Holds every work item, reservation, agent, schedule, cost record and the map of the program."}</p>
        <div class="intro-db">${t ? "ArangoDB" : "Database"} <small>${t ? "documents, change counters, the map as a graph, GraphRAG vectors" : "in the same container"}</small></div></div>
      <div class="intro-wire"><span class="intro-arr">&#8646;</span>${t ? "same API<br>engine token" : "hands out work,<br>records results"}</div>
      <div class="intro-engs">
        ${eng(1, chip("intro-ok", "agent: code") + chip("intro-ok", "agent: plan"))}
        ${eng(2, chip("intro-ok", "agent: code") + chip("intro-d", "free slot"))}
      </div></div>`;
  }

  // "it plugs into your repo, not your project": what iter touches, and what it never does
  function repoPlug() {
    const t = S.lv >= 3, b = S.lv >= 2;
    return `<div class="intro-plug">
      <div class="intro-plug-col intro-plug-no"><span class="intro-role">your project</span><h4>Untouched</h4>
        <ul><li>your application and how it runs</li><li>its build and its dependencies</li><li>its servers, its data, its users</li></ul>
        ${b ? `<p class="intro-g-biz">No library to install, no code to change, nothing to deploy with your product. Remove iter and your software doesn't notice.</p>` : ""}
        ${t ? `<p class="intro-g-tech">No SDK, no runtime hook, no build step: iter's code never runs inside your application.</p>` : ""}</div>
      <div class="intro-plug-mid"><span class="intro-plug-bolt">&#9889;</span><b>iter plugs in here</b></div>
      <div class="intro-plug-col intro-plug-yes"><span class="intro-role">your repository</span><h4>A few files beside the code</h4>
        <ul><li>short description files for the map</li><li>one small settings folder</li><li>ordinary git commits from the agents</li></ul>
        ${b ? `<p class="intro-g-biz">Works with any language or stack, on the repositories you already have; everything it adds is plain text you can read and review.</p>` : ""}
        ${t ? `<p class="intro-g-tech"><code>*.iter.md</code> node files · <code>main.iter.md</code> · <code>.iter/config.json</code> (+ a token in <code>.env</code>) · the engine works in a checkout through git pull / commit / push. iter_data stores records, not your code: node-file summaries and hashes, plus the chunks of documents you choose to index for search.</p>` : ""}</div>
    </div>`;
  }

  // what it is built from: contexts above crates
  function buildMap() {
    const t = S.lv >= 3, b = S.lv >= 2;
    const crate = (name, what, whyBiz, techLine, g) => `<div class="intro-crate">
      <div class="intro-crate-h"><code>${name}</code>${t && g ? gbtn(g, "graph") : ""}</div>
      <div>${what}</div>${b && whyBiz ? `<div class="intro-g-biz">${whyBiz}</div>` : ""}${t && techLine ? `<div class="intro-g-tech">${techLine}</div>` : ""}</div>`;
    return `<div class="intro-ctxs">
      <div class="intro-ctx intro-ctx-data"><div class="intro-ctx-h"><span class="intro-role">context</span><b>Orchestration and data</b><span class="intro-dim">the central side: one per team</span></div>
        ${crate("iter_data", "The data server: the API, the database, the map, search.", "Everything people and agents see comes from here.", "axum; ArangoDB storage; auth + roles; locks; versioned writes; graph; GraphRAG; MCP at /mcp", "iter_data")}
        ${crate("webui", "This web page.", "Watch, steer and answer from any browser.", "plain JS, embedded in the iter_data binary", "webui")}
      </div>
      <div class="intro-ctx intro-ctx-eng"><div class="intro-ctx-h"><span class="intro-role">context</span><b>Engines and checkout tools</b><span class="intro-dim">the working side: one per machine</span></div>
        ${crate("iter_engine", "The engine, and the <code>iter</code> command agents use.", "Runs the agents where the code lives; nothing leaves your machines but results.", "tick loop; claude -p sessions; close gate; datasync; GraphRAG worker (ingest, OCR, summaries)", "iter_engine")}
        ${crate("iter_local", "Reads and writes the project's files on disk.", "Keeps the map and the tests in the repository, under version control.", "node-file scan, ids, map snapshot, validate, test runner, graph edits", "iter_local")}
      </div>
      <div class="intro-ctx intro-ctx-shared"><div class="intro-ctx-h"><span class="intro-role">context</span><b>Shared rules and types</b><span class="intro-dim">used by both sides, so they never disagree</span></div>
        <div class="intro-crates-row">
        ${crate("iter_core", "The rulebook: what a work item is, when a reservation counts, when a schedule fires.", "", "pure functions: states, priorities, waits, dedup keys, schedules", "iter_core")}
        ${crate("iter_rag", "Reads documents and turns passages into searchable fingerprints.", "", "extract (pdf, docx, pptx…), token-sized chunks, all-MiniLM-L6-v2 embedder", "iter_rag")}
        </div>
      </div>
      ${t ? `<div class="intro-ctx intro-ctx-ship"><div class="intro-ctx-h"><span class="intro-role">context</span><b>Build, ship and prove</b></div>
        <div class="intro-small"><code>docker/</code> the all-in-one image (ArangoDB + iter_data) · <code>deploy.sh</code> docker | local · <code>e2e.sh</code> a real server and engine end to end</div></div>` : ""}
    </div>`;
  }

  /* ------------------------------------------------------------ the story */
  const STORY = [
    {
      kicker: "iter",
      title: "A team of AI coding agents on your software, with a foreman",
      lede: () => grow(["You describe the work. iter hands it to Claude Code agents running on your machines, keeps them out of each other's way, checks what they deliver, and asks you when it matters."]),
      body: () => `<div class="intro-hero intro-hero-c"><div class="intro-c-wrap">${coverSvg()}${coverPins()}</div><div class="intro-hero-side">
        ${biz(`Getting a project going, step by step:${coverStory()}`)}
        <div class="intro-chapters"><span class="intro-dim intro-small">the story</span>${["the work loop", "sharing the code", "done means checked", "people steer", "where it runs", "what it's built from"].map((c, i) => `<button class="intro-chap" data-go="${i + 1}">${c}</button>`).join("")}</div>
        ${tech(`<ul><li>A Rust workspace: <code>iter_data</code> (axum + ArangoDB, one container) and <code>iter_engine</code>, which runs headless <code>claude -p</code> sessions in each checkout.</li><li>One HTTP API for the page, the engines and agents; the same calls as MCP tools at <code>/mcp</code>.</li><li>Only engines touch your code; only iter_data touches the database.</li></ul>`)}
      </div></div>`
    },
    {
      kicker: "1 · the work loop",
      title: "Every piece of work travels the same loop",
      lede: () => grow(["A <b>work item</b> is like a ticket: a title, the request, a priority, the folders it may change, and a history of everything that happened to it.", "It loops: finished work often files the next work."]),
      body: () => `<div class="intro-loopwrap">${loopSvg()}${loopLegend()}</div>`
    },
    {
      kicker: "2 · sharing the code",
      title: "Many agents, one codebase, no collisions",
      lede: () => grow(["Three simple rules let a dozen agents share one codebase: reserve the folders you change, wait for the work you need, and go in priority order."]),
      body: () => `<div class="intro-three">
        <div class="intro-third"><h4>Reserve</h4>${lockTree()}<p class="intro-small">${grow(["An item reserves the folders it will change; another item that needs them waits.", "Like booking a meeting room: a crashed machine's booking expires on its own.", "lock rows {path, workid, expires} in iter_data; overlap = ancestor/descendant; leases renewed while the run lives; lock shape per agent refuses out-of-bounds reservations."])}</p></div>
        <div class="intro-third"><h4>Wait</h4>${depChain()}<p class="intro-small">${grow(["An item can wait for another, and for everything that one created.", "Work happens in the right order without anyone watching.", "blockedby (deep) · blockedby_shallow · cycles refused with the path named · agents declare waits mid-run (workitem_wait / iter wait --on)."])}</p></div>
        <div class="intro-third"><h4>Go in order</h4>${priorityBands()}<p class="intro-small">${grow(["The lowest number that is free to run goes next.", "A whole piece of work shares one number, so it moves through together.", "children inherit the creator's priority exactly; a root takes the lowest unused number in its band."])}</p></div>
      </div>
      ${biz(`When something waits, the queue says why: which reservation, which item, which limit. Nobody has to guess.`)}
      ${tech(`Visible waits: <code>blockedby_locks</code> names the holder, and one engine-owned <code>blocked by: …</code> tag gives the reason (lock, usage cap, agent cap, reservation, retry, approval, budget).`)}`
    },
    {
      kicker: "3 · done means checked",
      title: "An agent stopping is not the work being finished",
      lede: () => grow(["Before anything counts as done, iter checks it, and says so when it can't be sure."]),
      body: () => gateDiagram() + `<div class="intro-cols">
        ${biz(`<b>Honest reports are cheap.</b> Every agent ends with what it delivered and a <code>NOT DONE:</code> line for anything it didn't finish. An honest gap is one quick retry; a hidden one is exactly what the checker catches.`)}
        ${biz(`<b>Red tests become work.</b> Test groups run on a schedule; a group that turns red files <b>exactly one</b> item to fix it, never one per run.`)}
      </div>
      ${tech(`Not bounces: an incomplete verdict while declared blockers are open (the item queues behind them), and a verifier that fails twice (the item closes on the evidence, marked <code>unavailable</code>). Closed items are immutable except appended <code>doc</code> rows and a reopen by a person.`)}`
    },
    {
      kicker: "4 · people steer",
      title: "People steer; agents do the legwork",
      lede: () => grow(["You set the direction, the limits and the tie-breaks. iter enforces them."]),
      body: () => controlPanel() +
        at(2, `<div class="intro-usage"><div><h4>Several Claude accounts, used in turn</h4>${ladderBars()}</div>
          <p class="intro-small">${grow(["", "Each account has a <b>switch</b> level (move on to the next) and a <b>stop</b> level (never go past). Fewer agents run as usage climbs; when every account is at its stop level, iter pauses until usage resets.", "maxagents ladder by usage %, each agent type's max, accounts in order to switch % then round again to stop %; usage from stream-json rate_limit_event + a 1-token idle probe; engines avoid accounts other engines use."])}</p></div>`) +
        tech(code("project record: accounts, agent ladder, budget", JSON.stringify({
          maxagents: { ">98%": 0, ">95%": 1, ">90%": 2, "else": 4 }, maxdailycost: 50,
          accounts: [{ name: "Dev1", token_envar: "DEV1_TOKEN", order: 1, switch: 80, stop: 99 }, { name: "Dev2", token_envar: "DEV2_TOKEN", order: 2, switch: 80, stop: 99 }]
        }, null, 2)))
    },
    {
      kicker: "5 · where it runs",
      title: "It plugs into your repo, not your project",
      lede: () => grow(["Your software stays exactly as it is. iter works beside it, in the repository, through three parts: this web page, one central data server, and engines where the code lives."]),
      body: () => repoPlug() + archDiagram() +
        biz(`Only the engines touch your code, and only the data server touches the database. Your code stays in your git repository on machines you choose; the data server runs on a laptop or a small cloud machine, started with one command.`) +
        tech(`<ul><li>Each tick the engine reads the tiny <code>versions</code> counters and re-pulls only tables that moved; a full reload every 6 h is the fallback.</li>
          <li>A project's <code>state</code> (Running / Draining / Stopped) is the commanded state; each engine's heartbeat is the actual one.</li>
          <li>Graph edits from the page and GraphRAG work reach engines through the heartbeat reply, outside the work queue.</li></ul>`)
    },
    {
      kicker: "6 · what it's built from",
      title: "Two sides and a shared rulebook",
      lede: () => grow(["The central side keeps the records; the working side does the work where the code lives; both are built on the same rules, so they never disagree."]),
      body: () => buildMap()
    },
    {
      kicker: "your turn",
      title: "Start a project, or look around",
      lede: () => grow(["Everything in this story is live: the queue, the map and the search are the real thing, running on iter's own code."]),
      body: () => `<div class="intro-cta">
        <button class="intro-bigbtn" data-page="wizard"><b>Start a new project</b><span>name it, set its accounts and limits, and get the exact setup commands</span></button>
        <button class="intro-bigbtn intro-bigbtn-g" data-queue="1"><b>See the work queue</b><span>every item, its state and why it waits</span></button>
        <button class="intro-bigbtn intro-bigbtn-g" data-graph="iter4 harness"><b>Explore the project graph</b><span>iter4's own map: every part and how they connect</span></button>
        <button class="intro-bigbtn intro-bigbtn-g" data-page="glossary"><b>Read the glossary</b><span>every word on these slides, in one place</span></button>
      </div>`
    }
  ];

  /* ------------------------------------------------------------ the glossary */
  const GLOSSARY = [
    ["The work", [
      ["work item", "One unit of work: a title, the request, a priority, the folders it may change, tags, and a history (replies, checks, notes, cost). Like a ticket."],
      ["state", "Where an item is: queued, in-progress, question, complete, failed, parked, paused or scheduled."],
      ["question", "A state and a small form: the item waits for a person's decision; answering sends it back in line."],
      ["priority", "0–99, lower goes first: 0–9 now, 10–39 one number per use case, 40–49 people's requests, 50+ maintenance. Children inherit it."],
      ["dependency (blocked by)", "An item that waits for another item and everything that item created."],
      ["exec item", "An item that runs a shell command instead of an agent."],
      ["scheduled item", "A template that files a fresh item every time it fires (every N minutes, daily, weekly). Missed times are skipped, not caught up."],
      ["repeat (dedup)", "Filing the same fault again (same check: + container: tags) records a repeat on the open item instead of a second item."]]],
    ["Agents and engines", [
      ["agent", "A named kind of worker (code, plan, test, usecase, explain, summary…): its prompt, model, limits, check and allowed folders."],
      ["engine", "The iter program on one machine: it runs agents in its copy of the repository and reports to the data server."],
      ["data server (iter_data)", "The central server: every record, the API, the map, search; the only thing that touches the database."],
      ["session", "One headless Claude Code run (claude -p) doing an item's work, in several turns."],
      ["account / switch / stop", "A Claude subscription token the engine may use; switch % moves to the next account, stop % is never passed."],
      ["ELI5", "The Explain button on an item: a read-only agent re-explains it for someone new."]]],
    ["Sharing the code", [
      ["lock (reservation)", "An item's hold on the folders it changes while it runs; others that need them wait. Expires on its own if the machine dies."],
      ["lockdirs / lock shape", "The folders an item will change; the rules for which folders an agent may reserve."],
      ["reservation (slots)", "A hold that keeps room for a high-priority item that needs the whole tree."],
      ["Draining", "A project told to stop that still has work running; it becomes Stopped when that finishes."]]],
    ["Quality", [
      ["close gate", "The checks between 'the agent stopped' and 'complete': the facts, then a second AI's opinion."],
      ["bounce", "A close-gate failure that sends the item back once with the gaps listed."],
      ["NOT DONE line", "What an agent writes for anything it did not finish: an honest gap is cheap."],
      ["test group / sweep", "Scripts that check a part; the sweep runs them on a schedule and files one fix item per red group."]]],
    ["The map", [
      ["node file", "A short description file next to the code (name.<type>.iter.md): one point on the map."],
      ["project graph", "The map of the program drawn from its node files: parts, connections, requirements, tests."],
      ["context / container / component", "Map levels: a big area (data, engines), a deployable piece (a crate), a part inside it."],
      ["interface", "The contract between two parts: one operation, with an example of the data."],
      ["use case", "A journey someone takes through the product, and the parts it needs."],
      ["bizreq / techreq", "Business and technical requirements, one testable line each."],
      ["datasync", "How a change made on the map in the browser reaches the repository: the first free engine writes it."]]],
    ["Search and tools", [
      ["GraphRAG", "Search by meaning and by keyword over the project's documents and node files; each hit brings its place in the map."],
      ["chunk", "A passage of a document, sized for the search model; each has a summary and two fingerprints (vectors)."],
      ["Summary agent", "A small, fast model that summarises chunks, chapters and whole documents, outside the work queue."],
      ["embedding model", "all-MiniLM-L6-v2: turns text into 384 numbers so similar meanings sit close together."],
      ["MCP", "Model Context Protocol: iter's API as ready-made tools for AI assistants; every agent session has them."],
      ["user guide", "iter's own manual, searchable from every project."]]]
  ];
  function glossaryHtml() {
    const f = (S.gq || "").trim().toLowerCase();
    const groups = GLOSSARY.map(([g, terms]) => {
      const rows = terms.filter(([t, d]) => !f || (t + " " + d).toLowerCase().includes(f));
      return rows.length ? `<section class="intro-gsec"><h3>${esc(g)}</h3><dl class="intro-dl">${rows.map(([t, d]) => `<dt>${esc(t)}</dt><dd>${esc(d)}</dd>`).join("")}</dl></section>` : "";
    }).join("");
    return `<div class="intro-gfilter"><input id="intro-gq" type="search" placeholder="filter the glossary" value="${esc(S.gq || "")}" autocomplete="off"></div>
      <div class="intro-gloss-grid">${groups || `<p class="intro-dim">No term matches "${esc(f)}".</p>`}</div>
      <p class="intro-dim intro-small">Looking for how to do something? The GraphRAG tab searches iter's user guide from any project.</p>`;
  }

  /* ------------------------------------------------------------ the new-project wizard */
  const WIZ_SLIDE = { kicker: "new project", title: "Start a new project", wizard: true };
  const WSTEPS = ["Project", "Accounts & limits", "Engine", "Review", "Set up"];

  function freshWizard() {
    return {
      step: 0, topdirTouched: false, busy: false, error: "", created: null, token: null, tokenErr: "",
      d: {
        name: "", desc: "", gitrepo: "", topdir: "",
        accounts: [{ name: "Dev1", token_envar: "DEV1_TOKEN", switch: 80, stop: 99 }],
        ladder: [[">98%", 0], [">95%", 1], [">90%", 2], ["else", 4]],
        maxdailycost: "",
        engine: "Engine01",
        dataUrl: (typeof location !== "undefined" && /^https?:/.test(location.origin)) ? location.origin : "http://127.0.0.1:8300"
      }
    };
  }

  function slug(name) {
    let out = "";
    for (const c of name) {
      if (/[A-Za-z0-9_-]/.test(c)) out += c;
      else if (!out.endsWith("-")) out += "-";
    }
    return out.replace(/^-+|-+$/g, "");
  }
  const oneLine = s => String(s || "").split(/\s+/).filter(Boolean).join(" ");
  const yq = s => '"' + String(s).replace(/"/g, "'") + '"';

  function validate(w, step) {
    const d = w.d, e = {};
    if (step === 0) {
      if (!d.name.trim()) e.name = "A name is required.";
      else if (!/^[A-Za-z0-9][A-Za-z0-9 _.-]{0,63}$/.test(d.name.trim())) e.name = "Letters, digits, space, dot, dash and underscore only; start with a letter or digit; at most 64 characters.";
      else if (!slug(d.name.trim())) e.name = "Needs at least one letter or digit.";
      if (d.gitrepo.trim() && !/^(https?:\/\/|git@|ssh:\/\/)/.test(d.gitrepo.trim())) e.gitrepo = "Expected an https://, ssh:// or git@ URL.";
      if (!d.topdir.trim()) e.topdir = "Where the checkout lives on the engine machine.";
    }
    if (step === 1) {
      d.accounts.forEach((a, i) => {
        if (!a.name.trim()) e["acct" + i] = "Each account needs a name.";
        else if (!/^[A-Z_][A-Z0-9_]*$/.test(a.token_envar.trim())) e["acct" + i] = "The env var name: capitals, digits and underscores, e.g. DEV1_TOKEN.";
        else if (!(a.switch >= 0 && a.switch <= 100 && a.stop >= 0 && a.stop <= 100)) e["acct" + i] = "Percentages are 0–100.";
        else if (a.switch > a.stop) e["acct" + i] = "Switch should not be above stop.";
      });
      const names = d.accounts.map(a => a.name.trim());
      if (new Set(names).size !== names.length) e.accounts = "Account names must be different.";
      d.ladder.forEach(([k, v], i) => {
        if (!/^(else|[<>]=?\d{1,3}%)$/.test(String(k).trim())) e["lad" + i] = "Write a condition like >95% (or else).";
        else if (!(Number.isInteger(+v) && +v >= 0 && +v <= 64)) e["lad" + i] = "A whole number of agents, 0–64.";
      });
      if (!d.ladder.some(([k]) => String(k).trim() === "else")) e.ladder = "Add an else row: the agent count when no condition matches.";
      if (String(d.maxdailycost).trim() !== "" && !(+d.maxdailycost >= 0)) e.maxdailycost = "A dollar amount, 0 or more, or blank for no limit.";
    }
    if (step === 2) {
      if (!/^[A-Za-z0-9_.-]{1,64}$/.test(d.engine.trim())) e.engine = "Letters, digits, dot, dash and underscore only.";
      if (!/^https?:\/\/\S+$/.test(d.dataUrl.trim())) e.dataUrl = "An http:// or https:// URL the engine machine can reach.";
    }
    return e;
  }

  function projectRecord(d) {
    const rec = {
      name: d.name.trim(), desc: d.desc.trim(), state: "Stopped", gitrepo: d.gitrepo.trim(),
      maxagents: Object.fromEntries(d.ladder.map(([k, v]) => [String(k).trim(), +v])),
      agents: {},
      failure: { maxattempts: 5, first_retry_second: 10, retry_backoff_exponent: 2 },
      engines: [d.engine.trim()],
      accounts: d.accounts.map((a, i) => ({ name: a.name.trim(), token_envar: a.token_envar.trim(), order: i + 1, switch: +a.switch, stop: +a.stop }))
    };
    if (String(d.maxdailycost).trim() !== "") rec.maxdailycost = +d.maxdailycost;
    return rec;
  }
  function newEngineRecord(d) {
    return {
      name: d.engine.trim(), host: "", state: "Stopped", ticksec: 5, full_refresh_minutes: 360,
      queuelock: { retryms: 50, breaksec: 60 },
      projects: { [d.name.trim()]: { dirs: { topdir: d.topdir.trim() } } }
    };
  }
  // same shape iter_engine/src/init.rs writes
  function mainIterMd(d) {
    const name = d.name.trim(), s = slug(name), desc = oneLine(d.desc) || `The ${name} project.`;
    return `---
id: <a new uuid, minted by iter init>
projectname: ${yq(name)}
projectdescription: ${yq(desc)}
globalscandirs: ["{topdir}/"]
globalinterfacedir: "{topdir}/interfaces/"
globalusecasedir: "{topdir}/usecases/"
globalcontextfiles: ["{topdir}/reqs/${s}.bizreq.iter.md", "{topdir}/reqs/${s}.techreq.iter.md"]
children:
  codenodes: []
---

# ${name}

${desc}

This body is the first thing every agent reads about the project: keep it
current. Say what the project is, who it serves and the shape of the build.
Add one \`*.code.iter.md\` file per part of the code (a context, its
containers, their components) and list the top ones under
\`children.codenodes\` above; put shared contracts in \`interfaces/\` and the
journeys people take through the product in \`usecases/\`.
`;
  }
  const configJson = d => JSON.stringify({ data_url: d.dataUrl.trim(), token_envar: "ITER_ENGINE_TOKEN", engine_name: d.engine.trim(), env_file: "./.env" }, null, 2) + "\n";
  const shq = s => /^[A-Za-z0-9_./:@%+=~-]+$/.test(s) ? s : "'" + String(s).replace(/'/g, "'\\''") + "'";
  function initCmd(d) {
    let c = `iter_engine cli init --project ${shq(d.name.trim())} --data-url ${shq(d.dataUrl.trim())} --engine ${shq(d.engine.trim())}`;
    if (oneLine(d.desc)) c += ` \\\n  --desc ${shq(oneLine(d.desc))}`;
    return c;
  }
  function envLines(d, token) {
    const lines = ["# .env in the checkout (keep it out of git)", `ITER_ENGINE_TOKEN=${token || "<engine token, minted below>"}`];
    d.accounts.forEach(a => lines.push(`${a.token_envar.trim()}=<long-lived token for ${a.name.trim()}: run 'claude setup-token' while logged in to that account>`));
    if (!d.accounts.length) lines.push("# no accounts listed: the engine uses the machine's own Claude login");
    return lines.join("\n");
  }

  /* ------------------------------------------------------------ the module */

  function field(id, label, input, help, err) {
    return `<div class="intro-field"><label for="intro-f-${id}">${label}</label>${input}` +
      (err ? `<p class="intro-err">${esc(err)}</p>` : "") + `<p class="intro-help">${help}</p></div>`;
  }
  const inp = (id, val, attrs, bad) => `<input id="intro-f-${id}" data-f="${id}" value="${esc(val)}" ${attrs || ""} class="${bad ? "intro-bad" : ""}">`;

  function wizardHtml() {
    const w = S.w, d = w.d, admin = !!(S.ctx && S.ctx.isAdmin);
    const e = w.errs || {};
    const chips = WSTEPS.map((s, i) => `<span class="intro-stepchip ${i === w.step ? "intro-on" : i < w.step ? "intro-done" : ""}">${i + 1} · ${s}</span>`).join("");
    let body = "";
    if (w.step === 0) {
      body = field("name", "Project name", inp("name", d.name, 'placeholder="e.g. shop-api" autocomplete="off"', e.name),
          "The project's id everywhere: the queue, the graph, the engine, the file names. Short and stable; it cannot be renamed later.", e.name) +
        field("desc", "Description", `<textarea id="intro-f-desc" data-f="desc" placeholder="What the project does, for whom, and the shape of the build.">${esc(d.desc)}</textarea>`,
          "One paragraph, handed to <b>every</b> agent as context at the start of every session. It also becomes the opening of <code>main.iter.md</code>.") +
        field("gitrepo", "Git repository URL", inp("gitrepo", d.gitrepo, 'placeholder="https://github.com/you/shop-api"', e.gitrepo),
          "Where the engine pulls from and pushes to. Recorded for people and agents; the engine uses the checkout's own git remote.", e.gitrepo) +
        field("topdir", "Checkout path on the engine machine", inp("topdir", d.topdir, 'placeholder="~/dev/shop-api/"', e.topdir),
          "The folder holding the cloned repository on the machine that runs the engine (<code>topdir</code>). Agents work here; every lock path is relative to it.", e.topdir);
    } else if (w.step === 1) {
      const acctRows = d.accounts.map((a, i) => `<div class="intro-row intro-acct">
          <label class="intro-cell"><span class="intro-cellcap">name</span><input data-acct="${i}" data-k="name" value="${esc(a.name)}" aria-label="account name"></label>
          <label class="intro-cell"><span class="intro-cellcap">token env var</span><input data-acct="${i}" data-k="token_envar" value="${esc(a.token_envar)}" aria-label="token env var"></label>
          <label class="intro-cell intro-acct-sw"><span class="intro-cellcap">switch %</span><input type="number" min="0" max="100" data-acct="${i}" data-k="switch" value="${esc(a.switch)}" aria-label="switch percent"></label>
          <label class="intro-cell intro-acct-st"><span class="intro-cellcap">stop %</span><input type="number" min="0" max="100" data-acct="${i}" data-k="stop" value="${esc(a.stop)}" aria-label="stop percent"></label>
          <button class="intro-x" data-delacct="${i}" title="remove account">&times;</button></div>` + (e["acct" + i] ? `<p class="intro-err intro-small" style="color:var(--bad);margin:0">${esc(e["acct" + i])}</p>` : "")).join("");
      const ladRows = d.ladder.map(([k, v], i) => `<div class="intro-row intro-ladder">
          <input data-lad="${i}" data-k="0" value="${esc(k)}" aria-label="condition">
          <input type="number" min="0" data-lad="${i}" data-k="1" value="${esc(v)}" aria-label="agents">
          <button class="intro-x" data-dellad="${i}" title="remove row">&times;</button></div>` + (e["lad" + i] ? `<p class="intro-small" style="color:var(--bad);margin:0">${esc(e["lad" + i])}</p>` : "")).join("");
      body = `<h3>Claude accounts</h3>
        <p class="intro-dim intro-small">Each row is one Claude subscription. The token itself never leaves the engine machine: the project only names the <b>environment variable</b> in the engine's <code>.env</code> that holds it. Accounts are used top to bottom: each until its usage passes <b>switch</b> %, then, once all have, each again up to <b>stop</b> %; when every account is at stop, iter pauses until usage resets. With no rows, the engine uses the machine's own Claude login.</p>
        <div class="intro-rows"><div class="intro-row intro-acct intro-rowhead intro-accthead"><span>name</span><span>token env var</span><span>switch %</span><span>stop %</span><span></span></div>${acctRows}</div>
        ${e.accounts ? `<p class="intro-small" style="color:var(--bad)">${esc(e.accounts)}</p>` : ""}
        <button class="intro-btn intro-ghost intro-sm" data-addacct="1">+ account</button>
        <h3>Max agents ladder</h3>
        <p class="intro-dim intro-small">How many agents may run at once, by usage. Rows are checked top to bottom against the higher of the 5-hour and 7-day usage; the first true row wins, and <code>else</code> applies when none does.</p>
        <div class="intro-rows"><div class="intro-row intro-ladder intro-rowhead"><span>when usage is</span><span>agents</span><span></span></div>${ladRows}</div>
        ${e.ladder ? `<p class="intro-small" style="color:var(--bad)">${esc(e.ladder)}</p>` : ""}
        <button class="intro-btn intro-ghost intro-sm" data-addlad="1">+ row</button>
        <h3>Budget</h3>` +
        field("maxdailycost", "Max daily cost ($)", inp("maxdailycost", d.maxdailycost, 'inputmode="decimal" placeholder="blank = no limit"', e.maxdailycost),
          "Blank = no limit. <code>0</code> = spend nothing (a kill switch). Any other number stops new work once the day's recorded spend reaches it.", e.maxdailycost);
    } else if (w.step === 2) {
      const known = S.engines.map(x => x.name).filter(Boolean);
      const exists = known.includes(d.engine.trim());
      body = field("engine", "Engine name", inp("engine", d.engine, 'list="intro-englist" autocomplete="off"', e.engine) +
          `<datalist id="intro-englist">${known.map(n => `<option value="${esc(n)}">`).join("")}</datalist>`,
          (exists ? `<b>${esc(d.engine.trim())}</b> already exists: this project is added to it, its other projects are left alone. ` : (known.length ? `Existing engines: ${known.map(esc).join(", ")}. A new name creates a new engine record. ` : "A new engine record is created. ")) +
          "One engine is one <code>iter_engine</code> process on one machine; it can serve several projects. The name also becomes the engine's login user.", e.engine) +
        field("dataUrl", "Data server URL", inp("dataUrl", d.dataUrl, 'autocomplete="off"', e.dataUrl),
          "The address of this iter_data server <b>as the engine machine sees it</b>. Defaults to this page's address; change it if the engine reaches the server by another name.", e.dataUrl) +
        `<p class="intro-dim intro-small">The engine record stores the checkout path for this project (<code>projects.${esc(d.name.trim())}.dirs.topdir = ${esc(d.topdir.trim())}</code>). Heartbeat, state and account are filled in by the engine once it runs.</p>`;
    } else if (w.step === 3) {
      body = `<p>Nothing is written until you press <b>Create project</b>. The project starts <b>Stopped</b>. Two things must happen before any agent runs: an <b>engine</b> (the <code>iter_engine</code> program, on the machine that holds the code) must be running for it, and you press <b>Running</b> in the work queue. The next page gives the engine setup as one command.</p>` +
        code(`PUT /api/projects/${d.name.trim()}`, JSON.stringify(projectRecord(d), null, 2)) +
        code(`PUT /api/engines/${d.engine.trim()} (${S.engines.some(x => x.name === d.engine.trim()) ? "merged into the existing record" : "new record"})`,
          JSON.stringify({ projects: { [d.name.trim()]: { dirs: { topdir: d.topdir.trim() } } } }, null, 2)) +
        `<p class="intro-dim intro-small"><code>agents: {}</code> means every agent uses its global defaults; override them later from the project's gear. <code>failure</code>: up to 5 attempts, retrying after 10 s, then 20 s, 40 s…</p>` +
        (admin ? "" : `<div class="intro-note">Only an admin can create a project. Ask an admin to run this wizard, or sign in as one. You can still preview the setup commands.</div>`) +
        (w.error ? `<div class="intro-note intro-badn">${esc(w.error)}</div>` : "");
    } else {
      body = resultsHtml();
    }
    const foot = w.step < 3
      ? `<button class="intro-btn intro-ghost" data-wback="1" ${w.step === 0 ? "disabled" : ""}>Back</button><span class="intro-grow"></span><button class="intro-btn" data-wnext="1">Next</button>`
      : w.step === 3
        ? `<button class="intro-btn intro-ghost" data-wback="1">Back</button><span class="intro-grow"></span>` +
          (admin ? "" : `<button class="intro-btn intro-ghost" data-wpreview="1">Preview setup commands</button>`) +
          `<button class="intro-btn" data-wcreate="1" ${admin && !w.busy ? "" : "disabled"}>${w.busy ? "Creating…" : "Create project"}</button>`
        : `<button class="intro-btn intro-ghost" data-wreset="1">Start another project</button><span class="intro-grow"></span>${qbtn("Go to the work queue")}`;
    return `${w.existing ? "" : `<div class="intro-steps">${chips}</div>`}${body}<div class="intro-wizfoot">${foot}</div>`;
  }

  // where the setup script lives: this server (always matches it), or GitHub
  const SETUP_GH = "https://raw.githubusercontent.com/Stephen-Hilton/iter/main/iter4/tools/iter_engine_setup.sh";
  const setupUrl = d => d.dataUrl.trim().replace(/\/+$/, "") + "/iter_engine_setup.sh";
  function setupCmd(d, token) {
    const lines = [`bash iter_engine_setup.sh --data-url ${shq(d.dataUrl.trim())} --project ${shq(d.name.trim())} --engine ${shq(d.engine.trim())}`,
      `  --topdir ${shq(d.topdir.trim())}`,
      `  --token ${token ? shq(token) : "<the token from step 1>"}`];
    if (oneLine(d.desc)) lines.push(`  --desc ${shq(oneLine(d.desc))}`);
    lines.push("  --start");
    return `curl -fsSLo iter_engine_setup.sh ${setupUrl(d)}\n` + lines.join(" \\\n");
  }
  /** The engine's check-in state: online when it heartbeat within the last minute. */
  function engineState(rec) {
    if (!rec) return { online: false, text: "has not checked in yet" };
    const t = Date.parse(rec.last_seen || "");
    if (!t) return { online: false, text: "has never checked in" };
    const ago = Math.max(0, Math.round((Date.now() - t) / 1000));
    const when = ago < 90 ? `${ago} s ago` : ago < 5400 ? `${Math.round(ago / 60)} min ago` : `${Math.round(ago / 3600)} h ago`;
    return { online: ago < 60, text: `last checked in ${when}${rec.host ? " from " + rec.host : ""}` };
  }
  function watchHtml() {
    const d = S.w.d, eng = d.engine.trim();
    const st = engineState(S.engines.find(x => x.name === eng));
    return st.online
      ? `<div class="intro-note intro-okn"><b>Engine ${esc(eng)} is online</b> (${esc(st.text)}). Press <b>Running</b> on ${esc(d.name.trim())} in the work queue if it is Stopped; queued work starts within a few seconds.</div>`
      : `<div class="intro-note"><span class="intro-spin"></span> Waiting for engine <b>${esc(eng)}</b> to check in: it ${esc(st.text)}. This updates on its own.</div>`;
  }
  // re-read the engines every 5 s while the Set up page is on screen
  let watchTimer = null;
  function watchEngine() {
    if (watchTimer) return;
    watchTimer = setInterval(async () => {
      const el = S.el && S.el.querySelector("#intro-engwatch");
      if (!el || S.page !== "wizard" || S.w.step !== 4) { clearInterval(watchTimer); watchTimer = null; return; }
      if (!el.offsetParent) return; // the tab is hidden
      try { const r = await S.ctx.api("/api/engines"); S.engines = Array.isArray(r) ? r : S.engines; } catch (e) { return; }
      el.innerHTML = watchHtml();
    }, 5000);
  }

  function resultsHtml() {
    const w = S.w, d = w.d, admin = !!(S.ctx && S.ctx.isAdmin);
    const eng = d.engine.trim(), url = d.dataUrl.trim(), name = d.name.trim();
    let h = "";
    if (w.created) h += `<div class="intro-note intro-okn">Project <b>${esc(name)}</b> created (Stopped)${w.created.engineMsg ? "; " + esc(w.created.engineMsg) : ""}.</div>`;
    else if (!w.existing) h += `<div class="intro-note">Preview only: the project record was not created.</div>`;
    if (w.created && w.created.engineErr) h += `<div class="intro-note intro-badn">The engine record was not updated: ${esc(w.created.engineErr)}. Add the project to engine ${esc(eng)} from its gear in the work queue (checkout path ${esc(d.topdir.trim())}).</div>`;
    if (!eng) return h + `<div class="intro-note intro-badn">No engine is assigned to <b>${esc(name)}</b>. Add one from the engine gear in the work queue (projects served), then come back to this page.</div>`;
    const st = engineState(S.engines.find(x => x.name === eng));
    h += st.online
      ? `<p>Engine <b>${esc(eng)}</b> is already running (${esc(st.text)}), and it serves <b>${esc(name)}</b> from its next tick. It still needs the project's files in <code>${esc(d.topdir.trim())}</code> on that machine: run step 2 there without <code>--start</code>.</p>`
      : `<div class="intro-note"><b>Nothing runs yet.</b> Work items for ${esc(name)} stay <b>queued</b> until an engine is running for it. An engine is the <code>iter_engine</code> program, running on the machine that holds the code (<code>${esc(d.topdir.trim())}</code>): it takes queued work and runs the agents there. iter_data cannot start it for you. Three steps:</div>`;

    h += `<h3>1 · Mint the engine token</h3><p class="intro-dim intro-small">The engine signs in to this server as a user named <code>${esc(eng)}</code> (role <code>engine</code>). The token lasts a year and is shown once; step 2 puts it in the checkout's <code>.env</code> for you.</p>`;
    if (w.token) {
      h += `<div class="intro-note intro-okn">Token for <b>${esc(eng)}</b> minted, and filled into the command below.</div>` +
        `<div class="intro-actions"><button class="intro-btn intro-sm intro-ghost" data-copytoken="1">Copy the token alone</button></div>`;
    } else if (admin) {
      h += `<div class="intro-actions"><button class="intro-btn" data-wmint="1" ${w.busy ? "disabled" : ""}>Create user ${esc(eng)} and mint its token</button></div>`;
    } else {
      h += `<div class="intro-note">Only an admin can mint it: ask one to open this page (Intro → Start a new project, or the work queue's Engine setup link).</div>`;
    }
    if (w.tokenErr) h += `<div class="intro-note intro-badn">${esc(w.tokenErr)}</div>`;

    const accts = d.accounts.map(a => a.token_envar.trim()).filter(Boolean);
    h += `<h3>2 · Run the setup script on that machine</h3>
      <p class="intro-dim intro-small">Open a terminal on the machine that holds <code>${esc(d.topdir.trim())}</code> and paste this. The script checks the tools (git, curl, the <code>claude</code> CLI), finds <code>iter_engine</code> or builds it from GitHub, creates the folder, scaffolds the project files, writes <code>.env</code> and keeps it out of git, starts the engine and waits for it to check in. It never overwrites a file, so it is safe to run again.</p>` +
      code("in a terminal on the engine machine", setupCmd(d, w.token)) +
      `<ul class="intro-small intro-dim">
        <li>Claude accounts: ${accts.length ? `it asks for ${accts.map(a => `<code>${esc(a)}</code>`).join(", ")}, one token per account (run <code>claude setup-token</code> while logged in to it). Already have them in another env file? Add <code>--env-from ~/path/to/.env</code> and they are copied.` : "none listed, so the agents use that machine's own <code>claude</code> login."}</li>
        <li>Already built <code>iter_engine</code>? Add <code>--bin /path/to/iter_engine</code>. Otherwise the script builds it with <code>cargo</code> (install Rust from <a href="https://rustup.rs" target="_blank" rel="noopener">rustup.rs</a> first).</li>
        <li>That machine cannot reach ${esc(url)}? Then the <code>--data-url</code> above is wrong for it too: use the address it does reach this server by. The script itself is also on <a href="${esc(SETUP_GH)}" target="_blank" rel="noopener">GitHub</a>: <code>curl -fsSLo iter_engine_setup.sh ${esc(SETUP_GH)}</code></li>
        <li>Later: <code>bash iter_engine_setup.sh --status</code> or <code>--stop</code> in the checkout; the log is <code>.iter/engine.log</code>.</li>
      </ul>
      <div class="intro-actions"><a class="intro-btn intro-ghost intro-sm" href="${esc(setupUrl(d))}" download="iter_engine_setup.sh">Download iter_engine_setup.sh</a></div>`;

    h += `<h3>3 · Wait for it to check in</h3><div id="intro-engwatch">${watchHtml()}</div>`;

    h += `<details class="intro-byhand"><summary>Or do it by hand (what the script does)</summary>` +
      `<h4>Set up the checkout</h4><p class="intro-dim intro-small">Clone the repository if you haven't, then run this inside it. It writes the files below and never overwrites one that exists (add <code>--force</code> to replace them).</p>` +
      code("in the checkout", `cd ${shq(d.topdir.trim())}\n${initCmd(d)}`) +
      code("main.iter.md (the project's head file)", mainIterMd(d)) +
      code(".iter/config.json (how the engine reaches this server)", configJson(d)) +
      `<p class="intro-dim intro-small">Also written: <code>.iter/.gitignore</code> (users/, bin/, temp/), <code>reqs/${esc(slug(name))}.bizreq.iter.md</code>, <code>reqs/${esc(slug(name))}.techreq.iter.md</code>, and empty <code>interfaces/</code> and <code>usecases/</code> folders. Commit them.</p>` +
      (w.token ? "" : code("the engine token, with an admin token in $ADMIN_TOKEN",
        `curl -X PUT ${url}/api/users/${eng} -H "authorization: Bearer $ADMIN_TOKEN" -H 'content-type: application/json' \\\n  -d '{"role":"engine"}'      # only if the user does not exist yet\ncurl -X POST ${url}/api/users/${eng}/token -H "authorization: Bearer $ADMIN_TOKEN" -H 'content-type: application/json' -d '{}'`)) +
      `<h4>The .env file</h4><p class="intro-dim intro-small">In the checkout, next to <code>.iter/</code>. Keep it out of git. Keep <code>ANTHROPIC_API_KEY</code> out of it and out of the engine's environment: it outranks the account token and bills API credits instead.</p>` +
      code(".env", envLines(d, w.token)) +
      `<h4>Start the engine</h4>` +
      code("in the checkout", "iter_engine --accounts                  # check every account env var is set\niter_engine --config .iter/config.json  # start; the first line names this server") +
      `</details>`;
    watchEngine();
    return h;
  }

  /** The Set up page for a project that already exists (the work queue's "Engine setup" link):
   *  the same steps, filled from the project and its engine record. */
  async function openSetup(name) {
    if (!S.ctx || !S.ctx.api || !name) return;
    const api = S.ctx.api;
    let proj = null;
    try { proj = await api("/api/projects/" + encodeURIComponent(name)); } catch (e) { toast("Could not load project " + name); return; }
    try { const r = await api("/api/engines"); S.engines = Array.isArray(r) ? r : []; } catch (e) { S.engines = []; }
    const w = freshWizard(), d = w.d;
    const engs = proj.engines || [];
    // the engine to set up: the first assigned one that is not online, else the first
    const pick = engs.find(n => !engineState(S.engines.find(x => x.name === n)).online) || engs[0] || "";
    const rec = S.engines.find(x => x.name === pick);
    d.name = name; d.desc = proj.desc || ""; d.gitrepo = proj.gitrepo || ""; d.engine = pick;
    d.topdir = (rec && rec.projects && rec.projects[name] && rec.projects[name].dirs && rec.projects[name].dirs.topdir) || "";
    d.accounts = (proj.accounts || []).map(a => ({ name: a.name || "", token_envar: a.token_envar || "", switch: a.switch, stop: a.stop }));
    w.step = 4; w.existing = true;
    S.w = w; S.page = "wizard"; save(); render(true);
    if (S.el.getBoundingClientRect().top < 0) window.scrollTo({ top: 0 });
  }

  async function createProject() {
    const w = S.w, d = w.d, api = S.ctx.api, name = d.name.trim(), eng = d.engine.trim();
    w.busy = true; w.error = ""; render();
    try {
      let exists = false;
      try { await api("/api/projects/" + encodeURIComponent(name)); exists = true; } catch (e) { if (e.status !== 404) throw e; }
      if (exists) { w.error = `A project named "${name}" already exists. Pick another name, or edit that project from its gear in the work queue.`; return; }
      await api("/api/projects/" + encodeURIComponent(name), { method: "PUT", body: JSON.stringify(projectRecord(d)) });
      const created = { engineMsg: "", engineErr: "" };
      try {
        let rec = null;
        try { rec = await api("/api/engines/" + encodeURIComponent(eng)); } catch (e) { if (e.status !== 404) throw e; }
        if (rec) {
          rec.projects = rec.projects || {};
          const cur = rec.projects[name] || {};
          rec.projects[name] = Object.assign({}, cur, { dirs: Object.assign({}, cur.dirs || {}, { topdir: d.topdir.trim() }) });
          await api("/api/engines/" + encodeURIComponent(eng), { method: "PUT", body: JSON.stringify(rec) });
          created.engineMsg = `added to engine ${eng}`;
        } else {
          await api("/api/engines/" + encodeURIComponent(eng), { method: "PUT", body: JSON.stringify(newEngineRecord(d)) });
          created.engineMsg = `engine ${eng} created`;
        }
      } catch (e) { created.engineErr = e.message || String(e); }
      w.created = created; w.step = 4;
      try { S.ctx.reloadProjects && S.ctx.reloadProjects(); } catch (e) { /* the list refreshes on its own later */ }
    } catch (e) {
      w.error = "Could not create the project: " + (e.message || e);
    } finally { w.busy = false; render(); }
  }

  async function mintToken() {
    const w = S.w, eng = w.d.engine.trim(), api = S.ctx.api;
    w.busy = true; w.tokenErr = ""; render();
    try {
      let user = null;
      try { user = await api("/api/users/" + encodeURIComponent(eng)); } catch (e) { if (e.status !== 404) throw e; }
      if (user && user.role && user.role !== "engine") {
        w.tokenErr = `A user named "${eng}" already exists with role ${user.role}; an engine needs its own user with role engine. Pick another engine name, or mint by hand.`;
        return;
      }
      if (!user) await api("/api/users/" + encodeURIComponent(eng), { method: "PUT", body: JSON.stringify({ user: eng, role: "engine", email: "", settings: {}, authz: {} }) });
      const r = await api("/api/users/" + encodeURIComponent(eng) + "/token", { method: "POST", body: JSON.stringify({ ttl_days: 365 }) });
      w.token = r && r.token;
      if (!w.token) w.tokenErr = "The server answered without a token.";
    } catch (e) {
      w.tokenErr = "Could not mint the token: " + (e.message || e);
    } finally { w.busy = false; render(); }
  }

  async function loadEngines() {
    if (!S.ctx || !S.ctx.api) return;
    try { const r = await S.ctx.api("/api/engines"); S.engines = Array.isArray(r) ? r : []; } catch (e) { S.engines = []; }
    if (S.el && current().wizard && S.w.step >= 2 && S.w.step <= 3) render();
  }


  /* ------------------------------------------------------------ rendering */
  // page: story | glossary | wizard
  const S = { el: null, ctx: null, page: "story", i: 0, lv: 1, resetLv: false, gq: "", w: freshWizard(), engines: [], keyBound: false };
  const current = () => STORY[S.i];
  const save = () => store.set(POS_KEY, { page: S.page, i: S.i, lv: S.lv, resetLv: S.resetLv });

  function levelBar() {
    return `<div class="intro-detail" role="group" aria-label="level of detail">
      <span class="intro-detail-l">Detail</span>
      <div class="intro-stops">
        <div class="intro-stops-track"><div class="intro-stops-fill" style="width:${(S.lv - 1) * 50}%"></div></div>
        ${LEVELS.map(l => `<button class="intro-stop ${l.n <= S.lv ? "intro-on" : ""} ${l.n === S.lv ? "intro-cur" : ""}" data-lv="${l.n}" title="${esc(l.hint)}" aria-pressed="${l.n === S.lv}"><span class="intro-stop-dot"></span><span class="intro-stop-l">${l.label}</span></button>`).join("")}
      </div></div>`;
  }

  function render(focusSlide) {
    if (!S.el) return;
    // keep focus + caret in the field being typed in across re-renders
    const ae = document.activeElement, keep = ae && S.el.contains(ae) && ae.matches("input,textarea")
      ? { sel: ae.id ? "#" + ae.id : ae.dataset.acct != null ? `[data-acct="${ae.dataset.acct}"][data-k="${ae.dataset.k}"]` : ae.dataset.lad != null ? `[data-lad="${ae.dataset.lad}"][data-k="${ae.dataset.k}"]` : null, s: ae.selectionStart, e: ae.selectionEnd }
      : null;
    copies = [];
    const n = STORY.length;
    let main;
    if (S.page === "glossary") {
      main = `<section class="intro-slide intro-page" tabindex="-1">
        <p class="intro-kicker">glossary</p><h2 class="intro-title">Words you will meet</h2>
        <div class="intro-body">${glossaryHtml()}</div></section>`;
    } else if (S.page === "wizard") {
      main = `<section class="intro-slide intro-page" tabindex="-1">
        <p class="intro-kicker">${S.w.existing ? "engine setup" : esc(WIZ_SLIDE.kicker)}</p><h2 class="intro-title">${S.w.existing ? "Get " + esc(S.w.d.name.trim()) + " running" : WIZ_SLIDE.title}</h2>
        <div class="intro-body">${wizardHtml()}</div></section>`;
    } else {
      const sl = current();
      main = `<section class="intro-slide intro-lv${S.lv}" tabindex="-1" aria-live="polite">
        <p class="intro-kicker">${esc(sl.kicker)}</p>
        <h2 class="intro-title">${sl.title}</h2>
        <p class="intro-lede">${sl.lede()}</p>
        <div class="intro-body">${sl.body()}</div>
      </section>
      <div class="intro-nav">
        <button class="intro-arrow" data-go="${S.i - 1}" ${S.i === 0 ? "disabled" : ""} aria-label="previous slide">&larr;</button>
        <div class="intro-dots">${STORY.map((s, i) => `<button class="intro-dot ${i === S.i ? "intro-on" : ""}" data-go="${i}" title="${esc((i + 1) + ". " + s.kicker)}" aria-label="slide ${i + 1}: ${esc(s.kicker)}"></button>`).join("")}</div>
        <button class="intro-arrow" data-go="${S.i + 1}" ${S.i === n - 1 ? "disabled" : ""} aria-label="next slide">&rarr;</button>
      </div>
      <div class="intro-keys">&larr; &rarr; move between slides · &uarr; &darr; change the detail</div>
      <label class="intro-opt"><input type="checkbox" id="intro-resetlv" ${S.resetLv ? "checked" : ""}> Back to Summary on every new slide</label>`;
    }
    const pageBtn = (p, label) => `<button class="intro-btn ${S.page === p ? "" : "intro-ghost"} intro-sm" data-page="${S.page === p ? "story" : p}">${S.page === p ? "&larr; Back to the story" : label}</button>`;
    S.el.innerHTML = `<div class="intro-root">
      <div class="intro-bar">
        ${S.page === "story" ? levelBar() : ""}
        <span class="intro-where">${S.page === "story" ? `${S.i + 1} / ${n}` : ""}</span>
        <div class="intro-pages">${pageBtn("glossary", "Glossary")}${pageBtn("wizard", "Start a new project")}</div>
      </div>
      ${main}
    </div>`;
    if (keep && keep.sel) {
      const t = S.el.querySelector(keep.sel);
      if (t) { t.focus(); try { t.setSelectionRange(keep.s, keep.e); } catch (e) { /* number inputs */ } }
    } else if (focusSlide) {
      const s = S.el.querySelector(".intro-slide"); if (s) s.focus({ preventScroll: true });
    }
  }

  function go(i) {
    i = Math.max(0, Math.min(STORY.length - 1, i));
    if (S.page === "story" && i === S.i) return;
    // optional: each new slide starts at Summary, not the last slide's detail
    if (S.resetLv && S.page === "story") S.lv = 1;
    S.page = "story"; S.i = i; save(); render(true);
    if (S.el.getBoundingClientRect().top < 0) window.scrollTo({ top: 0 });
  }
  function setLevel(lv) {
    lv = Math.max(1, Math.min(3, lv));
    if (lv === S.lv) return;
    S.lv = lv; save(); render();
  }
  function setPage(p) {
    S.page = p === "glossary" || p === "wizard" ? p : "story";
    save(); render(true);
    if (S.el.getBoundingClientRect().top < 0) window.scrollTo({ top: 0 });
    if (S.page === "wizard") loadEngines();
  }

  function wizNext() {
    const w = S.w;
    w.errs = validate(w, w.step);
    if (Object.keys(w.errs).length) { render(); const bad = S.el.querySelector(".intro-bad,.intro-err"); if (bad) bad.scrollIntoView({ block: "center" }); return; }
    w.errs = {}; w.step++;
    if (w.step === 2 || w.step === 3) loadEngines();
    render(true);
  }

  function onClick(ev) {
    const b = ev.target.closest("button");
    if (!b || !S.el.contains(b)) return;
    const ds = b.dataset, w = S.w, d = w.d;
    if (ds.go != null) return go(+ds.go);
    if (ds.lv != null) return setLevel(+ds.lv);
    if (ds.page) return setPage(ds.page);
    if (ds.wizard) return setPage("wizard");
    if (ds.graph) { if (S.ctx && S.ctx.openGraph) S.ctx.openGraph(ds.graph); return; }
    if (ds.queue) { if (S.ctx && S.ctx.openQueue) S.ctx.openQueue(); return; }
    if (ds.copy != null) return copyText(copies[+ds.copy]);
    if (ds.copytoken) return copyText(w.token || "");
    if (ds.wnext) return wizNext();
    if (ds.wback) { w.step = Math.max(0, w.step - 1); w.error = ""; w.errs = {}; return render(true); }
    if (ds.wcreate) return createProject();
    if (ds.wpreview) { w.step = 4; w.created = null; return render(true); }
    if (ds.wmint) return mintToken();
    if (ds.wreset) { S.w = freshWizard(); return render(true); }
    if (ds.addacct) { const k = d.accounts.length + 1; d.accounts.push({ name: "Dev" + k, token_envar: "DEV" + k + "_TOKEN", switch: 80, stop: 99 }); return render(); }
    if (ds.delacct != null) { d.accounts.splice(+ds.delacct, 1); w.errs = {}; return render(); }
    if (ds.addlad) { const at = d.ladder.findIndex(([k]) => k === "else"); d.ladder.splice(at < 0 ? d.ladder.length : at, 0, [">80%", 3]); return render(); }
    if (ds.dellad != null) { d.ladder.splice(+ds.dellad, 1); w.errs = {}; return render(); }
  }

  // hovering a step in the loop's list lights its circle, and the reverse
  function onOver(ev) {
    const pin = ev.target.closest("[data-pin]");
    S.el.querySelectorAll(".intro-pinhot").forEach(x => x.classList.remove("intro-pinhot"));
    if (pin && S.el.contains(pin)) S.el.querySelectorAll(`[data-pin="${pin.dataset.pin}"]`).forEach(x => x.classList.add("intro-pinhot"));
    const t = ev.target.closest("[data-step]");
    S.el.querySelectorAll(".intro-hot").forEach(x => x.classList.remove("intro-hot"));
    if (!t || !S.el.contains(t)) return;
    S.el.querySelectorAll(`[data-step="${t.dataset.step}"]`).forEach(x => x.classList.add("intro-hot"));
  }

  function onInput(ev) {
    const t = ev.target;
    if (t.id === "intro-gq") {
      S.gq = t.value;
      const grid = S.el.querySelector(".intro-gloss-grid");
      if (grid) { const tmp = document.createElement("div"); tmp.innerHTML = glossaryHtml(); grid.replaceWith(tmp.querySelector(".intro-gloss-grid")); }
      return;
    }
    const d = S.w.d;
    if (t.dataset.f) {
      const f = t.dataset.f;
      d[f] = t.value;
      if (f === "topdir") S.w.topdirTouched = true;
      if (f === "name" && !S.w.topdirTouched) {
        d.topdir = t.value.trim() ? "~/dev/" + slug(t.value.trim()) + "/" : "";
        const td = S.el.querySelector("#intro-f-topdir"); if (td) td.value = d.topdir;
      }
      if (t.classList.contains("intro-bad")) t.classList.remove("intro-bad");
    } else if (t.dataset.acct != null) {
      const a = d.accounts[+t.dataset.acct]; if (!a) return;
      a[t.dataset.k] = t.type === "number" ? (t.value === "" ? NaN : +t.value) : t.value;
    } else if (t.dataset.lad != null) {
      const r = d.ladder[+t.dataset.lad]; if (!r) return;
      r[+t.dataset.k] = +t.dataset.k === 1 ? (t.value === "" ? NaN : +t.value) : t.value;
    }
    // once a Next has shown errors, re-check as the user fixes them
    if (S.w.errs && Object.keys(S.w.errs).length) {
      const before = JSON.stringify(S.w.errs);
      S.w.errs = validate(S.w, S.w.step);
      if (JSON.stringify(S.w.errs) !== before) render();
    }
  }

  function onChange(ev) {
    if (ev.target.id === "intro-resetlv") { S.resetLv = ev.target.checked; save(); ev.target.blur(); } // blur: the arrow keys ignore focused inputs
  }

  function visible() { return S.el && S.el.isConnected && S.el.offsetParent !== null; }
  function onKey(ev) {
    if (!visible() || ev.defaultPrevented || ev.altKey || ev.ctrlKey || ev.metaKey) return;
    const t = ev.target;
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
    if (document.querySelector("dialog[open]") || S.page !== "story") return;
    if (ev.key === "ArrowRight") { ev.preventDefault(); go(S.i + 1); }
    else if (ev.key === "ArrowLeft") { ev.preventDefault(); go(S.i - 1); }
    else if (ev.key === "ArrowUp") { ev.preventDefault(); setLevel(S.lv + 1); }
    else if (ev.key === "ArrowDown") { ev.preventDefault(); setLevel(S.lv - 1); }
  }
  let touch = null;
  function onTouchStart(ev) {
    if (ev.touches.length !== 1 || S.page !== "story" || ev.target.closest("input,textarea,pre,.intro-tablewrap,.intro-code")) { touch = null; return; }
    touch = { x: ev.touches[0].clientX, y: ev.touches[0].clientY };
  }
  function onTouchEnd(ev) {
    if (!touch) return;
    const dx = ev.changedTouches[0].clientX - touch.x, dy = ev.changedTouches[0].clientY - touch.y;
    touch = null;
    if (Math.abs(dx) > 60 && Math.abs(dx) > 2 * Math.abs(dy)) go(S.i + (dx < 0 ? 1 : -1));
  }

  window.IterIntro = {
    mount(el, ctx) {
      S.el = el; S.ctx = ctx || {};
      const pos = store.get(POS_KEY);
      if (pos) {
        S.page = ["story", "glossary", "wizard"].includes(pos.page) ? pos.page : "story";
        S.i = Math.max(0, Math.min(STORY.length - 1, pos.i | 0));
        S.lv = [1, 2, 3].includes(pos.lv) ? pos.lv : 1;
        S.resetLv = pos.resetLv === true;
      }
      el.addEventListener("click", onClick);
      el.addEventListener("input", onInput);
      el.addEventListener("change", onChange);
      el.addEventListener("mouseover", onOver);
      el.addEventListener("touchstart", onTouchStart, { passive: true });
      el.addEventListener("touchend", onTouchEnd, { passive: true });
      if (!S.keyBound) { document.addEventListener("keydown", onKey); S.keyBound = true; }
      render();
      if (S.page === "wizard") loadEngines();
    },
    // called whenever the tab is shown AND by the page's 10-second refresh:
    // never re-render here — a re-render would throw away what the reader has
    // open or half-typed; only a change of admin rights redraws the wizard
    /** The work queue's "Engine setup" link: the Set up steps for an existing project. */
    openSetup(name) { return openSetup(name); },
    show(ctx) {
      const wasAdmin = !!(S.ctx && S.ctx.isAdmin);
      if (ctx) S.ctx = ctx;
      if (!S.el || !S.el.firstChild) return render();
      if (S.page === "wizard" && wasAdmin !== !!(S.ctx && S.ctx.isAdmin)) render();
    }
  };
})();
