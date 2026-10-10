/* iter5 webui — the Intro tab (from iter4, rebuilt 2026-09-30; designer wizard 2026-10-02).
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
  const POS_KEY = "iter5.intro";

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
    [2, 30, 150, "your repo", "Design the project in the graph, then Build: an engine creates the repository and writes the files."],
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
    const tree = [[0, "Your_Repo/"], [1, "global/", "shop.project.iter.md"], [2, "requirements/", "*.bizreq / *.techreq"], [1, "src/"], [2, "data/", "data.code.iter.md"], [3, "postgres/", "+ postgres.code.iter.md"], [4, "tests/", "*.test.iter.md"],
      [2, "app/"], [3, "applogic/"], [4, "src/"], [1, ".iter/", "engine-only"]];
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
    { t: "Pick up", d: ["A free engine takes the most important item that can run now.", "Items that must wait say why in the queue: a reserved folder, a budget, another item.", "POST /api/projects/{p}/next: the server picks, claims and locks the item in one call; the engine checks its caps (maxagents ladder, agent max, budget, account) before it asks."] },
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
        ${t ? `<p class="intro-g-tech"><code>*.iter.md</code> node files (one <code>*.project.iter.md</code> head, code, test, requirement, use-case and actor nodes) · the engine keeps them and the project graph identical within seconds, and works in the checkout through git commit / push. One engine per machine, started with <code>iter_engine --data-url URL --env-file PATH</code>; everything else comes from the settings graph.</p>` : ""}</div>
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
        ${crate("iter_engine", "The engine, and the <code>iter</code> command agents use.", "Runs the agents where the code lives; nothing leaves your machines but results.", "one per machine, many projects; claude -p sessions; close gate; file sync + conform; build; GraphRAG worker", "iter_engine")}
        ${crate("iter_local", "Reads and writes the project's files on disk.", "Keeps the map and the tests in the repository, under version control.", "git-ignore-aware file walking, test runner (standard result JSON), validate", "iter_local")}
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
        tech(code("settings graph: a bills edge (account → project)", JSON.stringify({
          type: "bills", from: "account:Dev1", to: "project:shop", tag: "shop_default", settings: { order: 1, switch: 80, stop: 99, model: "" }
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
          <li>Graph edits from the page, builds and GraphRAG work reach engines through the heartbeat reply (<code>files_waiting</code>, <code>build_waiting</code>, <code>rag_waiting</code>), outside the work queue.</li></ul>`)
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
        <button class="intro-bigbtn" data-page="wizard"><b>Design a new project</b><span>name it, lay it out in the project graph, then Build it on an engine</span></button>
        <button class="intro-bigbtn intro-bigbtn-g" data-queue="1"><b>See the work queue</b><span>every item, its state and why it waits</span></button>
        <button class="intro-bigbtn intro-bigbtn-g" data-graph="iter_data"><b>Explore the project graph</b><span>every part, how they connect, and the connections between them</span></button>
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
      ["engine", "The iter program on one machine: it serves every project connected to it in the settings graph, runs their agents and keeps their files in sync."],
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
      ["context / container / component / connection", "Code levels: a big area, a deployable piece, a part inside it — and a connection, a type of link between them."],
      ["connection", "A node for a type of link (an API call over HTTP, an event, a stream): the parts that supply it, and the parts it connects to."],
      ["test node", "A test.iter.md file: the metadata for test scripts that print one standard result line (pass / fail counts)."],
      ["philosophy", "The highest-level guide: agents use it to infer missing requirements and settle conflicts."],
      ["actor", "Who drives the use cases, and where people touch the application."],
      ["designed", "A node that exists only in the project graph: the project has no repository yet. Build creates it."],
      ["use case", "A journey someone takes through the product, and the parts it needs."],
      ["bizreq / techreq", "Business and technical requirements, one requirement per file."],
      ["file sync", "Nodes and files are kept identical: an edit in the graph is written by an engine within seconds, and a file edited in the repository updates its node."],
      ["settings graph", "Every setting as a node or an edge: an engine serves a project, an account bills a project, an agent runs on a project. Edges can be switched off without losing their settings."]]],
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
  const WIZ_SLIDE = { kicker: "new project", title: "Design a new project", wizard: true };
  const WSTEPS = ["Project", "Review", "Design & build"];

  function freshWizard() {
    return {
      step: 0, busy: false, error: "", created: null, token: null, tokenErr: "",
      d: {
        name: "", desc: "", gitrepo: "", file_naming: "sequence", maxdailycost: "",
        engine: "", envFile: "~/.iter5/.env",
        dataUrl: (typeof location !== "undefined" && /^https?:/.test(location.origin)) ? location.origin : "http://127.0.0.1:8400"
      }
    };
  }

  function slug(name) {
    // spec §2.5: lowercase, [a-z0-9_-], runs of other characters become _, trimmed
    return String(name || "").toLowerCase().replace(/[^a-z0-9_-]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 60);
  }
  const shq = s => /^[A-Za-z0-9_./:@%+=~-]+$/.test(s) ? s : "'" + String(s).replace(/'/g, "'\\''") + "'";
  const shqPath = p => /^~\//.test(p) ? "~/" + (p.length > 2 ? shq(p.slice(2)) : "") : shq(p);

  function validate(w, step) {
    const d = w.d, e = {};
    if (step === 0) {
      if (!d.name.trim()) e.name = "A name is required.";
      else if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$/.test(d.name.trim())) e.name = "Letters, digits, dot, dash and underscore only; start with a letter or digit; at most 64 characters.";
      if (d.gitrepo.trim() && !/^(https?:\/\/|git@|ssh:\/\/)/.test(d.gitrepo.trim())) e.gitrepo = "Expected an https://, ssh:// or git@ URL.";
      if (String(d.maxdailycost).trim() !== "" && !(+d.maxdailycost >= 0)) e.maxdailycost = "A dollar amount, 0 or more, or blank for no limit.";
    }
    return e;
  }
  function projectRecord(d) {
    const rec = {
      name: d.name.trim(), desc: d.desc.trim(), state: "Stopped", gitrepo: d.gitrepo.trim(), file_naming: d.file_naming,
      maxagents: { ">98%": 0, ">95%": 1, ">90%": 2, "else": 4 },
      failure: { maxattempts: 5, first_retry_second: 10, retry_backoff_exponent: 2 }
    };
    if (String(d.maxdailycost).trim() !== "") rec.maxdailycost = +d.maxdailycost;
    return rec;
  }
  function engineCmd(d) {
    return `iter_engine --data-url ${shq(d.dataUrl.trim())} --env-file ${shqPath(d.envFile.trim() || "~/.iter5/.env")}` + (d.engine.trim() ? ` --name ${shq(d.engine.trim())}` : "");
  }
  function envLines(d, token) {
    return ["# the engine's env file — once per machine, never in a repository",
      `ITER_ENGINE_TOKEN=${token || "<the engine token, minted below>"}`,
      "# one line per LLM account this machine holds; the account node's token_envar names it",
      "CLAUDE_TOKEN_MAIN=<run 'claude setup-token' while logged in to that account>",
      "# anything else agents need (API keys for your own services…)"].join("\n");
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
      body = `<p class="intro-dim">A new project starts as a <b>design</b>: no repository, no engine. Lay it out in the Project graph — contexts, containers, components, the connections between them, requirements, use cases and actors — then press <b>Build</b> and an engine creates the repository and writes every file.</p>` +
        field("name", "Project name", inp("name", d.name, 'placeholder="e.g. shop-api" autocomplete="off"', e.name),
          `Also becomes the project's stable id, used in URLs, edges and work items. You can rename the project later (its id stays).${d.name.trim() ? ` Its head file: <code>{topdir}/global/${esc(slug(d.name.trim()) || "project")}.project.iter.md</code>.` : ""}`, e.name) +
        field("desc", "Description", `<textarea id="intro-f-desc" data-f="desc" placeholder="What the project does, for whom, and the shape of the build.">${esc(d.desc)}</textarea>`,
          "About 100 words, handed to <b>every</b> agent as context. It becomes the project node's <code>desc</code>.") +
        field("gitrepo", "Git remote (optional)", inp("gitrepo", d.gitrepo, 'placeholder="https://github.com/you/shop-api"', e.gitrepo),
          "When set, Build adds it as <code>origin</code> and the engine pushes after each commit. Leave blank for a local-only repository.", e.gitrepo) +
        field("file_naming", "File names on a clash", `<select id="intro-f-file_naming" data-f="file_naming"><option value="sequence" ${d.file_naming === "sequence" ? "selected" : ""}>sequence — ledger01, ledger02…</option><option value="uuid12" ${d.file_naming === "uuid12" ? "selected" : ""}>uuid12 — ledger_&lt;last 12 of the id&gt;</option></select>`,
          "Node files are named after the node; this decides how a second node with the same name is told apart.") +
        field("maxdailycost", "Max daily cost ($)", inp("maxdailycost", d.maxdailycost, 'inputmode="decimal" placeholder="blank = no limit"', e.maxdailycost),
          "Blank = no limit. <code>0</code> = spend nothing (a kill switch).", e.maxdailycost);
    } else if (w.step === 1) {
      body = `<p>Nothing is written until you press <b>Create the design</b>. iter_data then creates:</p>
        <ul class="intro-small"><li>the project record (state <b>Stopped</b>),</li><li>its project node <code>global/${esc(slug(d.name.trim()) || "project")}.project.iter.md</code>,</li><li>three default global requirements — one <b>philosophy</b>, one <b>bizreq</b>, one <b>techreq</b>, empty for you to fill in —</li></ul>
        <p class="intro-small intro-dim">All of them are <b>designed</b>: they live only in iter_data until you Build. Accounts, engines and agents are connected to the project in the <b>Settings</b> graph (bills, serves and runs edges).</p>` +
        code(`PUT /api/projects/${d.name.trim()}`, JSON.stringify(projectRecord(d), null, 2)) +
        (admin ? "" : `<div class="intro-note">Creating a project needs an admin, or a user allowed to create projects. Ask an admin if the server refuses.</div>`) +
        (w.error ? `<div class="intro-note intro-badn">${esc(w.error)}</div>` : "");
    } else {
      body = resultsHtml();
    }
    const foot = w.step === 0
      ? `<span class="intro-grow"></span><button class="intro-btn" data-wnext="1">Next</button>`
      : w.step === 1
        ? `<button class="intro-btn intro-ghost" data-wback="1">Back</button><span class="intro-grow"></span><button class="intro-btn" data-wcreate="1" ${w.busy ? "disabled" : ""}>${w.busy ? "Creating…" : "Create the design"}</button>`
        : `<button class="intro-btn intro-ghost" data-wreset="1">Design another project</button><span class="intro-grow"></span>${w.existing ? "" : `<button class="intro-btn" data-wopen="1">Open it in the Project graph</button>`}`;
    return `${w.existing ? "" : `<div class="intro-steps">${chips}</div>`}${body}<div class="intro-wizfoot">${foot}</div>`;
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
    const eng = S.w.d.engine.trim();
    const online = S.engines.filter(x => engineState(x).online);
    if (eng) {
      const st = engineState(S.engines.find(x => (x.id || x.name) === eng || x.name === eng));
      return st.online ? `<div class="intro-note intro-okn"><b>Engine ${esc(eng)} is online</b> (${esc(st.text)}).</div>`
        : `<div class="intro-note"><span class="intro-spin"></span> Waiting for engine <b>${esc(eng)}</b> to check in: it ${esc(st.text)}. This updates on its own.</div>`;
    }
    return online.length ? `<div class="intro-note intro-okn">Online now: ${online.map(x => `<b>${esc(x.name || x.id)}</b>`).join(", ")}.</div>`
      : `<div class="intro-note"><span class="intro-spin"></span> No engine is online yet. This updates on its own.</div>`;
  }
  let watchTimer = null;
  function watchEngine() {
    if (watchTimer) return;
    watchTimer = setInterval(async () => {
      const el = S.el && S.el.querySelector("#intro-engwatch");
      if (!el || S.page !== "wizard" || S.w.step !== 2) { clearInterval(watchTimer); watchTimer = null; return; }
      if (!el.offsetParent) return;
      try { const r = await S.ctx.api("/api/engines"); S.engines = Array.isArray(r) ? r : S.engines; } catch (e) { return; }
      el.innerHTML = watchHtml();
    }, 5000);
  }

  function resultsHtml() {
    const w = S.w, d = w.d, admin = !!(S.ctx && S.ctx.isAdmin), name = d.name.trim();
    let h = "";
    if (w.created) h += `<div class="intro-note intro-okn">The design <b>${esc(name)}</b> is created: its project node and three default requirements are in the Project graph, marked <b>designed</b>.</div>`;
    h += `<div class="intro-route">
      <div class="intro-routestep"><span class="intro-pinn">1</span><div><b>Design it</b><p class="intro-small">${w.existing ? "In" : "Open it in"} the Project graph: <b>+ Node</b> adds contexts, containers, components, connections, tests, requirements, use cases and actors; draw edges between them. Nothing touches a disk yet.</p>
        ${w.existing ? "" : `<button class="intro-btn intro-ghost intro-sm" data-wopen="1">Open ${esc(name)} in the Project graph</button>`}</div></div>
      <div class="intro-routestep"><span class="intro-pinn">2</span><div><b>Have an engine running</b><p class="intro-small">One <code>iter_engine</code> per machine serves every project connected to it. Set it up once (below); it registers itself with this server.</p></div></div>
      <div class="intro-routestep"><span class="intro-pinn">3</span><div><b>Build</b><p class="intro-small">The Project graph's <b>Build…</b> button: pick the engine and a folder. The engine creates the folder, runs <code>git init</code>, writes every node file and commits; optionally the <b>plan</b> agent is queued to build the project from its design.</p></div></div>
      <div class="intro-routestep"><span class="intro-pinn">4</span><div><b>Connect accounts and run</b><p class="intro-small">In the <b>Settings</b> graph draw a <b>bills</b> edge from an LLM account to the project (switch % and stop % live on that edge), then press <b>Running</b> on the work queue.</p>
        <button class="intro-btn intro-ghost intro-sm" data-wsettings="${esc(name ? "project:" + name : "")}">Open the settings graph</button></div></div>
    </div>`;

    h += `<h3>Set up an engine (once per machine)</h3>
      <p class="intro-dim intro-small">The engine needs only two things: this server's address and an env file holding its credentials. Everything else — which projects it serves, in which folders, with which accounts — comes from the settings graph.</p>`;
    h += field("engine", "Engine name (optional)", inp("engine", d.engine, 'list="intro-englist" placeholder="defaults to the machine\'s short hostname" autocomplete="off"'),
      `Also its login user. ${S.engines.length ? "Registered: " + S.engines.map(x => esc(x.id || x.name) + (x.id && x.name && x.name !== x.id ? " (" + esc(x.name) + ")" : "")).join(", ") + "." : "No engine is registered yet."}`) +
      `<datalist id="intro-englist">${S.engines.map(x => `<option value="${esc(x.id || x.name)}">`).join("")}</datalist>`;
    h += `<h4>1 · The engine token</h4><p class="intro-dim intro-small">The engine signs in as a user with role <code>engine</code>; its token goes in the env file as <code>ITER_ENGINE_TOKEN</code>. Shown once.</p>`;
    if (w.token) h += `<div class="intro-note intro-okn">Token for <b>${esc(d.engine.trim())}</b> minted and filled in below.</div><div class="intro-actions"><button class="intro-btn intro-sm intro-ghost" data-copytoken="1">Copy the token alone</button></div>`;
    else if (admin) h += `<div class="intro-actions"><button class="intro-btn" data-wmint="1" ${w.busy || !d.engine.trim() ? "disabled" : ""} title="${d.engine.trim() ? "" : "type the engine name above first"}">Create user ${esc(d.engine.trim() || "<engine name>")} and mint its token</button></div>`;
    else h += `<div class="intro-note">Only an admin can mint an engine token.</div>`;
    if (w.tokenErr) h += `<div class="intro-note intro-badn">${esc(w.tokenErr)}</div>`;
    h += `<h4>2 · The env file</h4>` + code(d.envFile.trim() || "~/.iter5/.env", envLines(d, w.token)) +
      `<p class="intro-dim intro-small">Keep <code>ANTHROPIC_API_KEY</code> out of it: it outranks the account token and bills API credits instead.</p>`;
    h += `<h4>3 · Start it</h4>` + field("envFile", "Env file path", inp("envFile", d.envFile, 'autocomplete="off" spellcheck="false"'), "On the engine machine.") +
      field("dataUrl", "This server, as the engine machine reaches it", inp("dataUrl", d.dataUrl, 'autocomplete="off" spellcheck="false"'), "Defaults to this page's address.") +
      `<div id="intro-setupcmd">${code("on the engine machine", engineCmd(d))}</div>`;
    h += `<h4>4 · Wait for it to check in</h4><div id="intro-engwatch">${watchHtml()}</div>
      <p class="intro-dim intro-small">Then connect it: <b>Build</b> from the Project graph (a new repository), or, for a repository that already exists on that machine, draw a <b>serves</b> edge from the engine to the project in the settings graph and set its <code>topdir</code>.</p>`;
    watchEngine();
    return h;
  }

  /** The engine setup page for a project that already exists (the work queue's "Engine setup" link). */
  async function openSetup(name) {
    if (!S.ctx || !S.ctx.api || !name) return;
    try { const r = await S.ctx.api("/api/engines"); S.engines = Array.isArray(r) ? r : []; } catch (e) { S.engines = []; }
    const w = freshWizard();
    w.d.name = name; w.step = 2; w.existing = true;
    S.w = w; S.page = "wizard"; save(); render(true);
    if (S.el.getBoundingClientRect().top < 0) window.scrollTo({ top: 0 });
  }

  async function createProject() {
    const w = S.w, d = w.d, api = S.ctx.api, name = d.name.trim();
    w.busy = true; w.error = ""; render();
    try {
      let exists = false;
      try { const list = await api("/api/projects"); exists = (list || []).some(p => p && ((p.id || p.name) === name || p.name === name)); } catch (e) { /* the PUT below answers */ }
      if (exists) { w.error = `A project named "${name}" already exists. Pick another name.`; return; }
      await api("/api/projects/" + encodeURIComponent(name), { method: "PUT", body: JSON.stringify(projectRecord(d)) });
      w.created = { at: Date.now() }; w.step = 2;
      try { if (S.ctx.selectProject) await S.ctx.selectProject(name); else if (S.ctx.reloadProjects) S.ctx.reloadProjects(); } catch (e) { /* the list refreshes on its own later */ }
    } catch (e) {
      w.error = "Could not create the project: " + (e.message || e);
    } finally { w.busy = false; render(); }
  }

  async function mintToken() {
    const w = S.w, eng = w.d.engine.trim(), api = S.ctx.api;
    if (!eng) return;
    w.busy = true; w.tokenErr = ""; render();
    try {
      let user = null;
      try { user = await api("/api/users/" + encodeURIComponent(eng)); } catch (e) { if (e.status !== 404) throw e; }
      if (user && user.role && user.role !== "engine") {
        w.tokenErr = `A user named "${eng}" already exists with role ${user.role}; an engine needs its own user with role engine. Pick another engine name.`;
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
    if (S.el && S.page === "wizard" && S.w.step === 2) render();
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
        <div class="intro-pages">${pageBtn("glossary", "Glossary")}${pageBtn("wizard", "Design a new project")}</div>
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
    if (ds.wmint) return mintToken();
    if (ds.wreset) { S.w = freshWizard(); return render(true); }
    if (ds.wopen) { if (S.ctx && S.ctx.openGraph) S.ctx.openGraph("", d.name.trim()); return; }
    if (ds.wsettings != null) { if (S.ctx && S.ctx.openSettings) S.ctx.openSettings(ds.wsettings); return; }
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
      if (t.classList.contains("intro-bad")) t.classList.remove("intro-bad");
      // the engine page: rewrite the command (and what Copy copies) and the mint button without a re-render
      if (S.w.step === 2 && ["engine", "envFile", "dataUrl"].includes(f)) {
        const box = S.el.querySelector("#intro-setupcmd");
        if (box) { const cmd = engineCmd(d); const btn = box.querySelector("[data-copy]"); if (btn) copies[+btn.dataset.copy] = cmd; box.querySelector("pre").textContent = cmd; }
        const mint = S.el.querySelector("[data-wmint]");
        if (mint) { mint.disabled = !d.engine.trim() || S.w.busy; mint.textContent = `Create user ${d.engine.trim() || "<engine name>"} and mint its token`; }
        const watch = S.el.querySelector("#intro-engwatch"); if (watch) watch.innerHTML = watchHtml();
      }
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
