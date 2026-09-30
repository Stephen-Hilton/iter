/* iter4 webui — the Intro tab (Phase 2, R15).
 *
 * Slide-like pages in two tracks (Business, Technical) with prev/next arrows,
 * keyboard left/right, swipe on phones and a progress dot row; the last slide of
 * both tracks is the New project wizard. Self-contained: index.html loads
 * intro.css + intro.js and calls
 *   IterIntro.mount(el, ctx)   once, with an empty <div> the tab owns
 *   IterIntro.show(ctx)        whenever the tab becomes visible again
 * ctx = {api(path, opts) -> Promise<json>, project, isAdmin,
 *        openGraph(nodeQuery), openQueue(), reloadProjects()}.
 * Every class is prefixed intro-; colours come from index.html's :root tokens.
 */
(function () {
  "use strict";

  /* ------------------------------------------------------------ helpers */
  const esc = s => String(s == null ? "" : s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const store = {
    get(k) { try { return JSON.parse(localStorage.getItem(k) || "null"); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (e) { /* private window: fine */ } }
  };
  const POS_KEY = "iter_intro_pos";

  let copies = []; // copy-button payloads for the current render
  function code(caption, text) {
    const i = copies.push(text) - 1;
    return `<div class="intro-code"><div class="intro-cap"><span>${esc(caption)}</span>` +
      `<button class="intro-btn intro-ghost intro-sm" data-copy="${i}">Copy</button></div><pre>${esc(text)}</pre></div>`;
  }
  const gbtn = (query, label) => `<button class="intro-btn intro-ghost intro-sm" data-graph="${esc(query)}" title="Open the Project graph at ${esc(query)}">&#9673; ${esc(label || query)}</button>`;
  const qbtn = label => `<button class="intro-btn intro-ghost" data-queue="1">${esc(label || "Open the work queue")}</button>`;
  const chip = (cls, t) => `<span class="intro-chip ${cls || ""}">${t}</span>`;
  function flow(steps, note) {
    const parts = steps.map((s, i) => `<div class="intro-step ${s.cls || ""}"><span class="intro-n">${i + 1}</span><b>${s.t}</b><span>${s.d}</span></div>`);
    return `<div class="intro-flow">${parts.join('<span class="intro-to">&rarr;</span>')}</div>` + (note ? `<div class="intro-loopnote">${note}</div>` : "");
  }
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

  /* ------------------------------------------------------------ diagrams */
  function archDiagram(tech) {
    const web = tech
      ? `<p>One page, <code>webui/index.html</code>, plain JavaScript, built into the iter_data binary. Queue, project graph, settings.</p>`
      : `<p>Where people watch the work, answer questions, set priorities and budgets.</p>`;
    const data = tech
      ? `<p>axum HTTP API. The only thing that touches the database (behind one storage trait). Users, roles, tokens, locks, versioned writes, the project graph.</p>
         <div class="intro-db">ArangoDB <small>the one store: documents, counters, the map as a graph, and GraphRAG vectors</small></div>`
      : `<p>Holds every work item, lock, agent definition, schedule, user and spend record, and the map of the program.</p>
         <div class="intro-db">Database <small>ArangoDB, usually in the same container</small></div>`;
    const eng = (n, agents) => `<div class="intro-node intro-eng"><span class="intro-role">${tech ? "iter_engine" : "engine"} · machine ${n}</span>
         <h4>${tech ? "Engine0" + n : "Engine " + n}</h4>
         <p>${tech ? "Polls every 5 s, claims items, runs <code>claude -p</code> in the checkout, commits, pushes." : "Runs agents in its copy of the repository."}</p>
         <div class="intro-agents">${agents}</div></div>`;
    return `<div class="intro-arch">
      <div class="intro-node intro-web"><span class="intro-role">${tech ? "webui" : "the looks"}</span><h4>${tech ? "Web page" : "Web page"}</h4>${web}</div>
      <div class="intro-wire"><span class="intro-arr">&#8646;</span>${tech ? "HTTPS JSON<br>+ bearer token" : "reads &amp;<br>steers"}</div>
      <div class="intro-node intro-data"><span class="intro-role">${tech ? "iter_data" : "the brains"}</span><h4>Data server</h4>${data}</div>
      <div class="intro-wire"><span class="intro-arr">&#8646;</span>${tech ? "same API<br>engine token" : "hands out work,<br>records results"}</div>
      <div class="intro-engs">
        ${eng(1, chip("intro-ok", "agent: code") + chip("intro-ok", "agent: plan"))}
        ${eng(2, chip("intro-ok", "agent: code") + chip("intro-d", "idle slot"))}
      </div></div>`;
  }

  // node files (HTML, so they stay readable on phones) -> sync -> a small graph (SVG)
  function mapSvg() {
    const files = ["main.iter.md", "iter_data.code.iter.md", "iter_engine.code.iter.md", "lock-acquire.interface.iter.md", "map-the-repo.usecase.iter.md"];
    // vertices: [x, y, colour, label, label side]
    const V = { m: [120, 26, "var(--dim)", "main", "r"], d: [70, 96, "var(--accent)", "iter_data", "l"], e: [166, 96, "var(--accent)", "iter_engine", "r"],
      i: [108, 158, "var(--warn)", "lock-acquire", "b"], u: [200, 150, "var(--q)", "use case", "b"] };
    const edge = (a, b, dash) => `<line class="intro-s-edge" x1="${V[a][0]}" y1="${V[a][1]}" x2="${V[b][0]}" y2="${V[b][1]}"${dash ? ' stroke-dasharray="4 3"' : ""}/>`;
    const lbl = (x, y, l, side) => side === "l" ? `<text class="intro-s-lbl" x="${x - 15}" y="${y + 4}" text-anchor="end">${l}</text>`
      : side === "r" ? `<text class="intro-s-lbl" x="${x + 15}" y="${y + 4}">${l}</text>`
      : `<text class="intro-s-lbl" x="${x}" y="${y + 26}" text-anchor="middle">${l}</text>`;
    const verts = Object.values(V).map(([x, y, c, l, side]) => `<circle class="intro-s-v" cx="${x}" cy="${y}" r="9" style="stroke:${c}"/>${lbl(x, y, l, side)}`).join("");
    return `<div class="intro-map">
      <div class="intro-mapfiles">${files.map(f => `<code>${f}</code>`).join("")}</div>
      <div class="intro-mapsync"><span class="intro-arr">&rarr;</span>sync</div>
      <svg class="intro-svg" viewBox="0 0 250 196" role="img" aria-label="the five node files become five connected vertices">
        ${edge("m", "d")}${edge("m", "e")}${edge("e", "i")}${edge("d", "i")}${edge("u", "e", 1)}
        ${verts}
      </svg></div>`;
  }

  function ladderBars() {
    const row = (name, used, sw, st, note) => `<div class="intro-barrow"><b>${name}</b>
      <div class="intro-track-bar"><div class="intro-fill" style="width:${used}%"></div>
      <div class="intro-mark intro-sw" style="left:${sw}%"></div><div class="intro-mark intro-st" style="left:${st}%"></div></div>
      <span class="intro-dim">${note}</span></div>`;
    return `<div class="intro-bars">
      ${row("Dev1", 83, 80, 99, "past switch → next")}
      ${row("Dev2", 41, 80, 99, "in use now")}
      ${row("Dev3", 12, 80, 99, "waiting")}
      <div class="intro-small intro-dim"><span style="color:var(--warn)">&#9646;</span> switch % &nbsp; <span style="color:var(--bad)">&#9646;</span> stop % &nbsp; bar = the higher of 5-hour and 7-day usage</div>
    </div>`;
  }

  function lockTree(tech) {
    const code = tech ? s => `<code>${s}</code>` : s => s;
    return `<div class="intro-tree">
      <div>${tech ? code("{topdir}/") : "your repository/"}</div>
      <div class="intro-ind1">${code("api/")} ${chip("intro-ok", "locked by: Add refund endpoint")}</div>
      <div class="intro-ind2">${code("api/payments/")} ${chip("intro-w", "waiting: Fix rounding (inside api/)")}</div>
      <div class="intro-ind1">${code("web/checkout/")} ${chip("intro-ok", "locked by: New checkout page")}</div>
      <div class="intro-ind1">${code("docs/")} ${chip("intro-d", "free")}</div>
    </div>`;
  }

  const STATES = [
    ["intro-a", "queued", "ready to run once what it waits on is finished"],
    ["intro-w", "in-progress", "an agent is working on it right now"],
    ["intro-q", "question", "waiting for a person to answer or approve"],
    ["intro-ok", "complete", "done, checked and closed"],
    ["intro-b", "failed", "ran out of attempts; a person should look"],
    ["intro-d", "parked", "held for a future condition (rare)"],
    ["intro-d", "paused", "a short manual hold, e.g. while editing"],
    ["intro-d", "scheduled", "a recurring template that files a fresh item on its schedule"]
  ];
  const statesDl = () => `<dl class="intro-dl">${STATES.map(([c, s, d]) => `<dt>${chip(c, s)}</dt><dd>${d}</dd>`).join("")}</dl>`;

  /* ------------------------------------------------------------ business track */
  const BIZ = [
    {
      kicker: "iter in one minute",
      title: "A team of AI coding agents working on your software, without tripping over each other",
      lede: "You describe the work. iter hands it to Claude Code agents running on your own machines, keeps them out of each other's way, checks their work before calling it done, and asks a person when it is unsure.",
      body: () => `<div class="intro-cols">
        ${card("intro-a", "1 · you", "File the work", "<p>Write a request: what to change and where. Or file one big request and let a planning agent break it into smaller items.</p>")}
        ${card("intro-o", "2 · agents", "Agents build it", "<p>Several agents work at once, each in the part of the code it has reserved.</p>")}
        ${card("intro-q", "3 · iter", "iter checks and records", "<p>Nothing counts as done until a checker agrees. Every step is written down.</p>")}
      </div>
      <div class="intro-actions">
        <button class="intro-btn" data-wizard="1">Start a new project</button>
        <button class="intro-btn intro-ghost" data-track="tech">Technical overview instead</button>
        ${qbtn("See the work queue")}
      </div>`
    },
    {
      kicker: "why it exists",
      title: "One AI agent is handy. Ten at once need a foreman.",
      lede: "Running many agents on one codebase goes wrong in predictable ways. iter is the foreman that prevents each one.",
      body: () => `<div class="intro-tablewrap"><table class="intro-table">
        <tr><th>Without a harness</th><th>With iter</th></tr>
        <tr><td>Two agents edit the same files and undo each other.</td><td><b>Locks:</b> each item reserves the folders it will change; others wait their turn.</td></tr>
        <tr><td>Work happens in the wrong order.</td><td><b>Dependencies:</b> an item waits until the work it needs is finished.</td></tr>
        <tr><td>An agent says "done" when it isn't.</td><td><b>Close gate:</b> a second AI compares the claim with the request before the item closes.</td></tr>
        <tr><td>A stuck agent goes unnoticed.</td><td><b>Retries</b> with a growing wait, then "failed"; unclear cases go to a person as a <b>question</b>.</td></tr>
        <tr><td>Spend creeps up unseen.</td><td><b>Caps:</b> a daily cost limit and usage limits for each Claude account.</td></tr>
        <tr><td>How the system fits together lives in people's heads.</td><td><b>Project graph:</b> a map of the program built from files in the repository.</td></tr>
      </table></div>`
    },
    {
      kicker: "what it does",
      title: "The work cycle, in plain words",
      lede: "Every piece of work is a <b>work item</b>: like a ticket, with a title, the request, a priority, the folders it may change, and a history of everything that happened to it.",
      body: () => flow([
        { t: "File", d: "someone (or an agent) files a work item" },
        { t: "Pick up", d: "an engine takes the most important item that is free to run", cls: "intro-hl" },
        { t: "Build", d: "a Claude Code agent does the work in a copy of your repository" },
        { t: "Check", d: "the close gate verifies the work matches the request", cls: "intro-q" },
        { t: "Save", d: "the change is committed to git and the item closes", cls: "intro-ok" }
      ], "&hellip; then the next item. Many engines and many agents run this cycle at the same time.") +
        `<p class="intro-dim">An agent can file new items too: a planning agent turns one large request into a set of smaller, ordered ones. Those children carry the parent's priority, so a whole piece of work moves through together.</p>`
    },
    {
      kicker: "the parts",
      title: "Three parts: the brains, the brawn and the looks",
      lede: "One central data server, any number of engines where the code lives, and this web page.",
      body: () => archDiagram(false) +
        `<p class="intro-dim">Only the engines ever touch your code, and only the data server touches the database. The data server can run on a laptop (Docker Desktop) or a small cloud machine, started with one command.</p>
        <div class="intro-actions">${gbtn("iter_data", "data server in the graph")}${gbtn("iter_engine", "engine in the graph")}${gbtn("webui", "web page in the graph")}</div>`
    },
    {
      kicker: "running in parallel",
      title: "Many agents, no collisions",
      lede: "Three simple rules let a dozen agents share one codebase.",
      body: () => `<div class="intro-cols">
        ${card("intro-a", "", "Locks", "<p>A lock is a reservation on a folder, like booking a meeting room. An item that needs a booked folder, or anything inside it, waits. Bookings expire unless renewed, so a crashed machine never holds a room forever.</p>")}
        ${card("intro-o", "", "Dependencies", "<p>Item B can say \"wait for A\". A only counts as finished when A <i>and everything A created</i> are finished.</p>")}
        ${card("intro-w", "", "Priorities", "<p>A number from 0 to 99; lower goes first. 0–9 do now · 10–39 one number per use case · 40–49 requests from people · 50–99 maintenance.</p>")}
      </div>${lockTree(false)}
      <p class="intro-dim">When an item waits, the queue says why: which lock, which cap, or which item it is waiting on.</p>`
    },
    {
      kicker: "quality",
      title: "Work is checked before it counts as done",
      lede: "\"The agent stopped\" is not the same as \"the work is finished\". iter checks the difference.",
      body: () => `<div class="intro-cols">
        ${card("intro-q", "", "The close gate", "<p>Before an item closes, iter checks the facts (did the agent run out of turns? did it commit anything?) and asks a second AI whether the final report covers everything in the request. If not, the item goes back for one more try with the gaps listed. After that, a person decides.</p>")}
        ${card("intro-o", "", "Honest reports", "<p>Every agent must end with what it delivered and a <code>NOT DONE:</code> line for anything it didn't finish. An honest gap is a cheap retry; a hidden one is what the checker exists to catch.</p>")}
        ${card("intro-b", "", "Red tests become work", "<p>Test groups run on a schedule. A group that goes red files <b>exactly one</b> work item to fix it, never one per run.</p>")}
      </div>`
    },
    {
      kicker: "control",
      title: "People steer; agents do the legwork",
      lede: "You set the direction, the limits and the tie-breaks. iter does the rest.",
      body: () => `<div class="intro-cols">
        ${card("intro-q", "", "A question inbox", "<p>When an agent or the checker needs a decision, the item moves to <b>question</b> and waits. Answering puts it back in line with your answer at the top of its instructions.</p>")}
        ${card("intro-a", "", "Controls", "<p>Run or stop a whole project (stopping lets running work finish first). <b>Run now</b> on one item. Reopen a closed item. Change priorities any time.</p>")}
        ${card("intro-w", "", "Budget and usage", "<p>A daily cost cap (blank = no limit, 0 = spend nothing). Several Claude accounts, each with a <b>switch</b> and a <b>stop</b> percentage; fewer agents run as usage climbs, and when every account is at its stop level, iter pauses until usage resets.</p>")}
      </div>${ladderBars()}`
    },
    {
      kicker: "the map",
      title: "The project graph: your software, drawn from its own files",
      lede: "Each part of the program has a short description file (<code>*.iter.md</code>) next to its code. iter reads them all and draws the map.",
      body: () => mapSvg() +
        `<p>The map shows the parts, the contracts between them (<b>interfaces</b>), the journeys people take through the product (<b>use cases</b>), the requirements, and the tests. It answers questions a folder listing cannot: <i>which part owns this folder? what does this use case touch? which tests cover this contract?</i> It keeps itself up to date: an engine redraws it within a minute of a file changing.</p>
        <div class="intro-actions">${gbtn("iter4 harness", "iter4's own map")}${gbtn("map-the-repo", "a use case: Map a repository")}</div>`
    },
    {
      kicker: "the work queue",
      title: "Reading the work queue",
      lede: "Every item is in exactly one state. Most of the time you will only see the first three.",
      body: () => statesDl() +
        `<p class="intro-dim">Above the list, one progress chip per use case shows how many of its items are done, so you can see each piece of work move toward complete.</p>
        <div class="intro-actions">${qbtn()}</div>`
    },
    {
      kicker: "common questions",
      title: "What leaders usually ask",
      lede: "",
      body: () => `<div class="intro-faq">
        <details open><summary>Where does our code go?</summary><p>It stays in your git repository and on the engine machines you choose. The data server holds the work records; for the map it keeps each description file's summary and a fingerprint of its text, and the text itself stays in git.</p></details>
        <details><summary>What does it cost to run?</summary><p>Agents run on Claude subscriptions (Pro, Max, Team or Enterprise) through long-lived account tokens. iter records the spend of every item and can cap spend per day.</p></details>
        <details><summary>What happens when an agent fails?</summary><p>It retries with a growing wait (5 attempts by default), then the item shows as failed for a person. Anything that depends on it waits underneath it and carries on once it is fixed.</p></details>
        <details><summary>Can we see what happened?</summary><p>Each item keeps its request, the agent's replies, the checker's verdicts, test logs and notes. A closed item cannot be edited, only annotated or reopened by a person.</p></details>
        <details><summary>Where does it run?</summary><p>The data server is one container (database plus server) on a laptop or a small cloud machine. Engines run on any machine with a copy of the repository and Claude Code.</p></details>
        <details><summary>Any licensing to check?</summary><p>The default database, ArangoDB Community Edition (3.12.5 and later), is licensed for non-commercial use and datasets up to 100 GB. Whether that covers your use is the owner's decision; the Enterprise image needs no code change, and iter can also run on SQLite.</p></details>
      </div>`
    },
    {
      kicker: "getting started",
      title: "From nothing to a first finished item",
      lede: "Four steps. The wizard on the next page does the second one for you.",
      body: () => flow([
        { t: "Data server", d: "start it with one command (Docker), on a laptop or a small cloud machine" },
        { t: "Project", d: "create the project record: name, repository, accounts, budget", cls: "intro-hl" },
        { t: "Engine", d: "on a machine with the repository and Claude Code, run the setup command and start the engine" },
        { t: "First item", d: "file a work item in the queue, press Run, and watch it go", cls: "intro-ok" }
      ]) + `<div class="intro-actions"><button class="intro-btn" data-wizard="1">Open the new project wizard</button>${qbtn()}</div>`
    }
  ];

  /* ------------------------------------------------------------ technical track */
  const TECH = [
    {
      kicker: "iter4 for engineers",
      title: "A Rust harness that loops headless Claude Code agents over a central work queue",
      lede: "iter4 is iter3 with a new data backend (ArangoDB), a graph of the program built from its <code>*.iter.md</code> files, a stable id on every one of those files, and a test sweep that turns red test groups into work items. One cargo workspace:",
      body: () => `<div class="intro-tablewrap"><table class="intro-table">
        <tr><th>Crate / dir</th><th>What it is</th><th></th></tr>
        <tr><td><code>iter_core</code></td><td>shared types: work items, projects, engines, agents, locks, widgets, schedules, dedup keys</td><td>${gbtn("iter_core", "graph")}</td></tr>
        <tr><td><code>iter_data</code></td><td>the data server: axum API, ArangoDB storage, auth, locks, versioned writes, the project graph, the iter3 migration</td><td>${gbtn("iter_data", "graph")}</td></tr>
        <tr><td><code>iter_engine</code></td><td>the local engine and the <code>iter</code> verbs agents call (<code>add</code>, <code>ask</code>, <code>wait</code>, <code>ids</code>, <code>sync</code>, <code>sweep</code>…)</td><td>${gbtn("iter_engine", "graph")}</td></tr>
        <tr><td><code>iter_local</code></td><td>checkout-side tools: node-file scan, stable ids, map snapshot, test groups, test runner, validate</td><td>${gbtn("iter_local", "graph")}</td></tr>
        <tr><td><code>webui/</code></td><td>this page, embedded into iter_data</td><td>${gbtn("webui", "graph")}</td></tr>
        <tr><td><code>docker/</code></td><td>the all-in-one image: ArangoDB CE + iter_data</td><td>${gbtn("docker", "graph")}</td></tr>
      </table></div>
      <div class="intro-actions"><button class="intro-btn intro-ghost" data-track="biz">Business overview instead</button><button class="intro-btn" data-wizard="1">Start a new project</button></div>`
    },
    {
      kicker: "architecture",
      title: "Data is the brains, the engine the brawn, the web page the looks",
      lede: "",
      body: () => archDiagram(true) + `<ul>
        <li><b>iter_data is the only component that talks to the database</b>; engines and the page use its HTTP API. It is the lock authority too: no lock files in the repo.</li>
        <li><b>The engine is the only component with repo access.</b> Its config holds just enough to reach iter_data (<code>.iter/config.json</code> + a token in <code>.env</code>); everything else is central, so many engines on many hosts share one queue.</li>
        <li>Each tick (5 s) the engine reads the tiny <code>versions</code> seq rows and only re-pulls tables whose counter moved, writes its heartbeat, and reloads everything every 6 h as a fallback.</li>
        <li>A project's <code>state</code> (Running / Draining / Stopped) is the <i>commanded</i> state; each engine's state is the <i>actual</i> one. Stopping drains: running work finishes, nothing new starts.</li>
      </ul><div class="intro-actions">${gbtn("iter4 harness", "the context node")}${gbtn("iter_data")}${gbtn("iter_engine")}</div>`
    },
    {
      kicker: "data model",
      title: "Collections, versioned writes, and the work item",
      lede: "Every row keeps its iter3 partition/sort key and JSON body: <code>{_key, pk, sk, version, expires, workid, body}</code>.",
      body: () => `<div class="intro-cols">
        ${card("intro-a", "", "Collections", `<p><code>workitem</code> (headers the list reads) and <code>workitem_detail</code> (request, responses, verify rows, test logs, docs), <code>agent</code>, <code>agent_tooling</code>, <code>project</code>, <code>engine</code>, <code>webui_user</code>, <code>lock</code>, <code>versions</code>, <code>spend</code>; plus the graph: <code>node</code> vertices and <code>link</code> edges.</p>`)}
        ${card("intro-o", "", "Three atomic operations", `<p><b>Versioned write:</b> the writer sends the version it read (<code>expect_version</code>); a mismatch is a 409 and the writer re-reads. <b>Lock acquire:</b> create if absent, expired or already this item's. <b>Seq bump:</b> an atomic +1. Every backend passes one contract test for all three.</p>`)}
      </div>` + code("a work item header (iter_core)", JSON.stringify({
        id: "01890a5d-ac70-7db8-8b5d-10505a42232f", version: 7, name: "Add refund endpoint", project: "demo",
        state: "queued", agent: "code", priority: 12, lockdirs: ["{topdir}/api/refunds/"],
        blockedby: ["184fa9a3-f967-4a98-9d8f-57152e7cbe64"], createdby: "plan (agent)", attempt: 1, gate_bounces: 0,
        tags: [{ text: "usecase:refunds" }]
      }, null, 2)) + statesDl()
    },
    {
      kicker: "the loop",
      title: "What one engine does, every tick",
      lede: "",
      body: () => flow([
        { t: "Pick", d: "highest priority queued item whose dependencies are done and whose lockdirs are free; caps and schedules first" },
        { t: "Claim + lock", d: "versioned queued→in-progress, then one lock row per lockdir (a lost race releases and defers)", cls: "intro-hl" },
        { t: "Session", d: "claude -p turns: prework → the request → postwork → agent memory → self-check" },
        { t: "Close gate", d: "deterministic checks + a verifier model", cls: "intro-q" },
        { t: "Commit", d: "git commit + push of the item's lock scope only; release locks", cls: "intro-ok" }
      ], "&hellip; next item. A finished session may take on a queued neighbour with the same lockdirs and use case (session chaining).") +
        `<ul>
        <li><b>Caps:</b> the <code>maxagents</code> ladder (by usage %), each agent type's <code>max</code>, the daily budget, every account at stop %.</li>
        <li><b>Prompt</b> (order kept for the prompt cache): agent prompt body → shared rules → capability index → close-gate paragraph → project context (<code>main.iter.md</code> + <code>globalcontextfiles</code>) → the work item → previous attempt → context files, listed for the agent to read, never inlined.</li>
        <li><b>Failure:</b> retry with back-off (<code>first_retry_second</code> × <code>retry_backoff_exponent</code>ⁿ) up to <code>maxattempts</code>, then <code>failed</code>.</li>
        <li><b>exec items</b> run a shell command (<code>exec_shell</code>) instead of an agent; exit 0 is their contract. Scheduled items clone one on each firing (skip, don't backfill).</li>
      </ul>`
    },
    {
      kicker: "sharing a codebase",
      title: "Locks, lock shape, dependencies",
      lede: "",
      body: () => lockTree(true) + `<ul>
        <li><b>Lock rows</b> live in iter_data: <code>{project, path, engine, workid, acquired, expires}</code>. Overlap is ancestor/descendant. Long runs extend <code>expires</code> (a lease); an expired row may be replaced by any engine.</li>
        <li><b>Visible waits:</b> a waiter's <code>blockedby_locks</code> names the holder, and one engine-owned <code>blocked by: …</code> tag says why it isn't running (lock, usage cap, agent cap, reservation, retry, approval, budget, accounts).</li>
        <li><b>Reservations:</b> a high-priority item that locks the whole tree drains the slots instead of starving forever.</li>
        <li><b>Lock shape</b> (per agent, overridable per project): <code>allow</code> patterns, <code>outside: refuse|warn</code>, <code>max_overlap</code>, <code>none</code>. iter_data refuses an out-of-shape lockdir with a 400 that names the rule.</li>
        <li><b>Dependencies are deep:</b> a blocker counts as done only when it and everything it created closed complete. Cycles are refused at write time with the path named. Agents declare blockers mid-run with <code>iter wait --on &lt;id&gt;</code>.</li>
      </ul><div class="intro-actions">${gbtn("lock-acquire", "lock-acquire interface")}</div>`
    },
    {
      kicker: "close gate",
      title: "Exit 0 is not the definition of done",
      lede: "The gate runs in the engine's close step for agent items that returned successfully.",
      body: () => `<div class="intro-cols">
        ${card("intro-a", "deterministic, free", "Evidence checks", "<p><b>turn cap</b> (cut off ≠ finished) · <b>open review</b> rows with no disposition · <code>requires_children</code> (a plan must file items) · <code>requires_commit</code> (code must move HEAD).</p>")}
        ${card("intro-q", "one extra turn", "LLM verifier", "<p><code>closegate.verify</code>: a model alias (default haiku), read-only tools, sees the request, the final message and the evidence; answers <code>{verdict: complete|incomplete|unclear, open: [...], reason}</code>. It judges done-ness, not quality.</p>")}
      </div>` + flow([
        { t: "complete", d: "close complete", cls: "intro-ok" },
        { t: "bounce", d: "under max_bounces (1): verify row, back to queued with the gaps in the next prompt" },
        { t: "question", d: "at the limit or unclear: a question widget (continue | accept)", cls: "intro-q" }
      ]) + `<p class="intro-dim">Not bounces: a verdict of incomplete while declared blockers are still open (the item queues behind them, no bounce), and a verifier that fails to run twice in a row (the item closes on the evidence, marked <code>unavailable</code>). Closed items are immutable except appended <code>doc</code> rows and a reopen by a person.</p>`
    },
    {
      kicker: "agents",
      title: "Agent records, prompts and the agent-side CLI",
      lede: "An <b>agent</b> is a named record (<code>plan</code>, <code>code</code>, <code>test</code>, <code>usecase</code>, …) with defaults a project can override key by key.",
      body: () => code("agent record (abridged)", JSON.stringify({
        name: "code", max: 4, childstate: "queued", timeoutsec: 3600, model: "opus", flags: "--dangerously-skip-permissions",
        closegate: { verify: "haiku", requires_children: false, requires_commit: true, max_bounces: 1 },
        lockshape: { allow: [], outside: "refuse", max_overlap: 3 }, promptbody: "…"
      }, null, 2)) + `<ul>
        <li><b>Agent tooling</b> rows are the shared text around agents: <code>shared</code> rules, <code>capability</code> docs (indexed, read on demand), <code>source</code>, <code>prepost</code> steps, the <code>critic</code> persona.</li>
        <li><b><code>iter</code></b> is on every agent's PATH (a shim to <code>iter_engine cli</code>): <code>add</code> (file a child item), <code>ask</code> (question a person), <code>reject</code>, <code>wait --on</code>, <code>block</code>, <code>doc</code>, <code>critreview</code>, <code>capability</code>, <code>status</code>.</li>
        <li>The <code>test</code> agent (formerly <code>testwriter</code>): with an <code>exec_shell</code> it runs tests deterministically, with no model; without one it writes tests.</li>
        <li><b>Agent memory:</b> one short <code>&lt;dir&gt;.agentmemory.iter.md</code> per codepath, refreshed after each run and read first by the next one.</li>
      </ul>`
    },
    {
      kicker: "accounts and cost",
      title: "Usage caps across several Claude accounts",
      lede: "Each account is a long-lived token (<code>claude setup-token</code>) in the engine's <code>.env</code>; the project record only names the env var.",
      body: () => ladderBars() + code("project record: accounts + ladder + budget", JSON.stringify({
        maxagents: { ">98%": 0, ">95%": 1, ">90%": 2, "else": 4 }, maxdailycost: 50,
        accounts: [{ name: "Dev1", token_envar: "DEV1_TOKEN", order: 1, switch: 80, stop: 99 }, { name: "Dev2", token_envar: "DEV2_TOKEN", order: 2, switch: 80, stop: 99 }]
      }, null, 2)) + `<ul>
        <li>Use accounts in <code>order</code> until each passes <b>switch</b> %; when all have, go round again up to <b>stop</b> %; when all are at stop, hold and watch for the 5-hour / 7-day reset.</li>
        <li>An engine avoids accounts other running engines are using (unless that leaves none).</li>
        <li>Usage comes from Claude Code's stream-json <code>rate_limit_event</code> per run, plus a 1-token idle probe that reads the rate-limit headers.</li>
        <li><code>maxdailycost</code>: absent = unlimited, <code>0</code> = spend nothing, &gt;0 = $ per day. Every item gets a spend row, cache tokens included.</li>
      </ul>`
    },
    {
      kicker: "the map",
      title: "Node files → the project graph",
      lede: "Any <code>name.&lt;nodetype&gt;.iter.md</code> file is a <b>node file</b> (the dot rule). Its frontmatter starts with a stable <code>id: &lt;uuid&gt;</code>; its <code>children:</code> lists are the edges.",
      body: () => `<dl class="intro-dl">
        <dt><code>main</code></dt><dd>one per project: <code>projectname</code>, <code>projectdescription</code>, scan / interface / use case dirs, <code>globalcontextfiles</code> every agent reads.</dd>
        <dt><code>code</code></dt><dd><code>level: context | container | component</code>: a context is a bucket (data, auth…), containers are deployables or crates, components their parts. <code>codedirs</code>, <code>codenodes</code>, <code>inputs</code>/<code>outputs</code> (interfaces), reqs, tests.</dd>
        <dt><code>interface</code></dt><dd>one operation per file (<code>request-reply | event | stream | dataset</code>); the body is an example of the data. A outputs I and B inputs I draws a connection A→B.</dd>
        <dt><code>usecase</code></dt><dd>a journey through the product and the code nodes it touches, with numbered flow steps.</dd>
        <dt><code>bizreq</code> / <code>techreq</code></dt><dd>business and technical requirements, one testable bullet each.</dd>
        <dt><code>tests</code></dt><dd>formerly <code>testgroup</code>: scripts to run, with <code>teststate</code> omit / include / block / inherit per chain.</dd>
      </dl>` + mapSvg() + code("keep the map current", "iter ids --fix     # give every node file an id (reuses the stored id when it knows the file)\niter sync          # push the snapshot: one vertex per file, one edge per children link\niter validate      # structure checks, missing / malformed ids") +
        `<p class="intro-dim">A running engine syncs on its own, at most once a minute when the tree changed. Read routes: <code>GET /api/projects/{p}/graph</code>, <code>…/graph/lookup</code>, <code>…/graph/owner?path=</code>, <code>…/graph/nodes/{id}/neighbors</code>.</p>
        <div class="intro-actions">${gbtn("iter4 harness", "context node")}${gbtn("graph-sync", "graph-sync interface")}${gbtn("map-the-repo", "Map a repository")}</div>`
    },
    {
      kicker: "tests",
      title: "The test sweep: red tests become exactly one work item",
      lede: "",
      body: () => flow([
        { t: "Read the map", d: "tests vertices, reached through each root's chains" },
        { t: "Honour teststate", d: "omit / include / block / inherit, per chain" },
        { t: "Run", d: "the deterministic runner (iter runtests)", cls: "intro-hl" },
        { t: "Record", d: "POST …/graph/nodes/{id}/testresult" },
        { t: "File if red", d: "one code item per red group, unless one is already open", cls: "intro-b" }
      ]) + `<ul>
        <li>The filed item locks the parent code node's <code>codedirs</code>, names the failing checks with their output tails, and carries the parent's use case tag and priority band.</li>
        <li>Repeats merge: an open item carrying the same <code>check:testgroup:&lt;id&gt;</code> + <code>container:</code> tags is booked as "seen again" instead of filed twice.</li>
        <li>Test logs live on the work item (<code>log_header</code> / <code>log_detail</code> rows, one pair per group per attempt), not in the tree.</li>
      </ul>` + code("install the sweep as a recurring exec item", "iter sweep                                  # run now\niter sweep --install-schedule --every 4h    # recurring (user token required)") +
        `<div class="intro-actions">${gbtn("red-test-becomes-workitem", "use case: red test becomes a work item")}</div>`
    },
    {
      kicker: "get started",
      title: "Stand it up",
      lede: "",
      body: () => code("1 · the data server (API + this page on :8300)", "cd iter4\n./deploy.sh docker     # ArangoDB + iter_data in one container\n# secrets ITER_ADMIN_PASSWORD, ITER_JWT_SECRET come from ../.env\n# ./deploy.sh local    # native iter_data against a dev ArangoDB on :8529") +
        `<p><b>2 · the project record:</b> the wizard on the last page (admins). It also gives you the exact commands below with your names filled in.</p>` +
        code("3 · in the checkout on the engine machine", "iter_engine cli init --project demo --data-url http://127.0.0.1:8300 --engine Engine01\n# writes main.iter.md, .iter/config.json, .iter/.gitignore, reqs/, interfaces/, usecases/\n\n# .env beside it (never commit it):\nITER_ENGINE_TOKEN=<minted by an admin: POST /api/users/Engine01/token>\nDEV1_TOKEN=<output of: claude setup-token>\n\niter_engine --accounts                  # which account env vars are set\niter_engine --config .iter/config.json  # start") +
        `<p><b>4 ·</b> press Run on the project, file a work item, watch it close. Keep <code>ANTHROPIC_API_KEY</code> out of the engine's environment: it outranks the account token and bills API credits instead.</p>
        <div class="intro-actions"><button class="intro-btn" data-wizard="1">Open the wizard</button>${qbtn()}</div>`
    },
    {
      kicker: "glossary",
      title: "Words you will meet",
      lede: "",
      body: () => `<dl class="intro-dl intro-small">
        <dt>work item</dt><dd>one unit of work: a header row (state, agent, priority, lockdirs, tags) plus detail rows (request, responses, verify, logs, docs).</dd>
        <dt>agent</dt><dd>a named worker definition: prompt body, model, caps, close gate and lock shape.</dd>
        <dt>engine</dt><dd>one iter_engine process on one host; serves one or more projects from their checkouts (<code>topdir</code>).</dd>
        <dt>lockdirs / lock</dt><dd>the folders an item will write; each becomes a lock row while it runs.</dd>
        <dt>reservation</dt><dd>a lock-like row that holds slots for a high-priority whole-tree item.</dd>
        <dt>close gate</dt><dd>the checks between "the agent stopped" and "complete".</dd>
        <dt>bounce</dt><dd>a gate failure that sends the item back to queued with feedback.</dd>
        <dt>question</dt><dd>a state and a widget: the item waits for a person's answer.</dd>
        <dt>seq</dt><dd>a per-project, per-table change counter engines poll to know what to re-pull.</dd>
        <dt>versioned write</dt><dd>a write that only lands if the version is still the one the writer read.</dd>
        <dt>maxagents ladder</dt><dd>usage-% gates, checked in order, that set how many agents may run.</dd>
        <dt>switch / stop</dt><dd>per-account usage % at which the engine moves on, and at which it gives up.</dd>
        <dt>exec item</dt><dd>an item that runs a shell command instead of an agent.</dd>
        <dt>scheduled</dt><dd>a recurring template that files a fresh item each time it fires.</dd>
        <dt>usecase tag</dt><dd><code>usecase:&lt;name&gt;</code>, inherited by every child, drives the progress chips.</dd>
        <dt>dedup tags</dt><dd><code>check:</code> + <code>container:</code>: a repeat of an open item is merged, not filed.</dd>
        <dt>node file</dt><dd>a <code>name.&lt;nodetype&gt;.iter.md</code> file; one vertex in the graph.</dd>
        <dt>teststate</dt><dd>whether a chain's tests run: omit, include, block or inherit.</dd>
        <dt>Draining</dt><dd>a project told to stop that still has work running.</dd>
      </dl>`
    }
  ];

  /* ------------------------------------------------------------ wizard */
  const WIZ_SLIDE = { kicker: "new project", title: "Start a new project", wizard: true };
  const TRACKS = { biz: { label: "Business", slides: BIZ.concat([WIZ_SLIDE]) }, tech: { label: "Technical", slides: TECH.concat([WIZ_SLIDE]) } };
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
  const S = { el: null, ctx: null, track: "biz", i: 0, w: freshWizard(), engines: [], keyBound: false };

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
      body = `<p>Nothing is written until you press <b>Create project</b>. The project starts <b>Stopped</b>, so no agent runs until you press Run in the work queue.</p>` +
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
    return `<div class="intro-steps">${chips}</div>${body}<div class="intro-wizfoot">${foot}</div>`;
  }

  function resultsHtml() {
    const w = S.w, d = w.d, admin = !!(S.ctx && S.ctx.isAdmin);
    const eng = d.engine.trim(), url = d.dataUrl.trim();
    let h = w.created
      ? `<div class="intro-note intro-okn">Project <b>${esc(d.name.trim())}</b> created (Stopped)${w.created.engineMsg ? "; " + esc(w.created.engineMsg) : ""}.</div>`
      : `<div class="intro-note">Preview only: the project record was not created.</div>`;
    if (w.created && w.created.engineErr) h += `<div class="intro-note intro-badn">The engine record was not updated: ${esc(w.created.engineErr)}. Add the project to engine ${esc(eng)} from its gear in the work queue (checkout path ${esc(d.topdir.trim())}).</div>`;
    h += `<h3>1 · Set up the checkout</h3><p class="intro-dim intro-small">On the engine machine, clone the repository if you haven't, then run this inside it. It writes the files below and never overwrites one that exists (add <code>--force</code> to replace them).</p>` +
      code("in the checkout", `cd ${shq(d.topdir.trim())}\n${initCmd(d)}`) +
      code("main.iter.md (the project's head file)", mainIterMd(d)) +
      code(".iter/config.json (how the engine reaches this server)", configJson(d)) +
      `<p class="intro-dim intro-small">Also written: <code>.iter/.gitignore</code> (users/, bin/, temp/), <code>reqs/${esc(slug(d.name.trim()))}.bizreq.iter.md</code>, <code>reqs/${esc(slug(d.name.trim()))}.techreq.iter.md</code>, and empty <code>interfaces/</code> and <code>usecases/</code> folders. Commit them.</p>`;
    h += `<h3>2 · The engine token</h3><p class="intro-dim intro-small">The engine signs in as a user named after it, with role <code>engine</code>. An admin mints its token once; it lasts a year.</p>`;
    if (w.token) {
      h += `<div class="intro-note intro-okn">Token for <b>${esc(eng)}</b>, shown once. Put it in the checkout's <code>.env</code> now.</div><div class="intro-token">${esc(w.token)}</div>` +
        `<div class="intro-actions"><button class="intro-btn intro-sm" data-copytoken="1">Copy token</button></div>`;
    } else if (admin) {
      h += `<div class="intro-actions"><button class="intro-btn" data-wmint="1" ${w.busy ? "disabled" : ""}>Create user ${esc(eng)} and mint its token</button></div>`;
    } else {
      h += `<div class="intro-note">Ask an admin to mint it, or use the commands below with an admin token.</div>`;
    }
    if (w.tokenErr) h += `<div class="intro-note intro-badn">${esc(w.tokenErr)}</div>`;
    if (!w.token) h += code("or by hand, with an admin token in $ADMIN_TOKEN",
      `curl -X PUT ${url}/api/users/${eng} -H "authorization: Bearer $ADMIN_TOKEN" -H 'content-type: application/json' \\\n  -d '{"role":"engine"}'      # only if the user does not exist yet\ncurl -X POST ${url}/api/users/${eng}/token -H "authorization: Bearer $ADMIN_TOKEN" -H 'content-type: application/json' -d '{}'`);
    h += `<h3>3 · The .env file</h3><p class="intro-dim intro-small">In the checkout, next to <code>.iter/</code>. Keep it out of git. Keep <code>ANTHROPIC_API_KEY</code> out of it and out of the engine's environment: it outranks the account token and bills API credits instead.</p>` +
      code(".env", envLines(d, w.token)) +
      `<h3>4 · Start the engine</h3>` +
      code("in the checkout", "iter_engine --accounts                  # check every account env var is set\niter_engine --config .iter/config.json  # start; the first line names this server") +
      `<p class="intro-dim intro-small">Then open the work queue, press <b>Run</b> on the project, and file a first work item. The engine maps the repository on its own; <code>iter sync</code> does it by hand.</p>`;
    return h;
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
  const slides = () => TRACKS[S.track].slides;
  const current = () => slides()[S.i];

  function render(focusSlide) {
    if (!S.el) return;
    // keep focus + caret in the wizard field being typed in across re-renders
    const ae = document.activeElement, keep = ae && S.el.contains(ae) && ae.matches("input,textarea")
      ? { sel: ae.id ? "#" + ae.id : ae.dataset.acct != null ? `[data-acct="${ae.dataset.acct}"][data-k="${ae.dataset.k}"]` : ae.dataset.lad != null ? `[data-lad="${ae.dataset.lad}"][data-k="${ae.dataset.k}"]` : null, s: ae.selectionStart, e: ae.selectionEnd }
      : null;
    copies = [];
    const sl = current(), n = slides().length;
    const content = sl.wizard ? wizardHtml() : sl.body();
    S.el.innerHTML = `<div class="intro-root">
      <div class="intro-bar">
        <div class="intro-tracks" role="tablist">${Object.entries(TRACKS).map(([k, t]) => `<button class="intro-track ${k === S.track ? "intro-on" : ""}" data-track="${k}" role="tab" aria-selected="${k === S.track}">${t.label}</button>`).join("")}</div>
        <span class="intro-where">${S.i + 1} / ${n} · ${esc(sl.kicker)}</span>
      </div>
      <section class="intro-slide" tabindex="-1" aria-live="polite">
        <p class="intro-kicker">${esc(sl.kicker)}</p>
        <h2 class="intro-title">${sl.title}</h2>
        ${sl.lede ? `<p class="intro-lede">${sl.lede}</p>` : ""}
        <div class="intro-body">${content}</div>
      </section>
      <div class="intro-nav">
        <button class="intro-arrow" data-go="${S.i - 1}" ${S.i === 0 ? "disabled" : ""} aria-label="previous slide">&larr;</button>
        <div class="intro-dots">${slides().map((s, i) => `<button class="intro-dot ${i === S.i ? "intro-on" : ""}" data-go="${i}" title="${esc((i + 1) + ". " + s.kicker)}" aria-label="slide ${i + 1}: ${esc(s.kicker)}"></button>`).join("")}</div>
        <button class="intro-arrow" data-go="${S.i + 1}" ${S.i === n - 1 ? "disabled" : ""} aria-label="next slide">&rarr;</button>
      </div>
      <div class="intro-keys">&larr; &rarr; keys move between slides</div>
    </div>`;
    if (keep && keep.sel) {
      const t = S.el.querySelector(keep.sel);
      if (t) { t.focus(); try { t.setSelectionRange(keep.s, keep.e); } catch (e) { /* number inputs */ } }
    } else if (focusSlide) {
      const s = S.el.querySelector(".intro-slide"); if (s) s.focus({ preventScroll: true });
    }
  }

  function go(i) {
    const n = slides().length;
    i = Math.max(0, Math.min(n - 1, i));
    if (i === S.i) return;
    S.i = i;
    store.set(POS_KEY, { track: S.track, i: S.i });
    render(true);
    if (S.el.getBoundingClientRect().top < 0) window.scrollTo({ top: 0 });
    if (current().wizard) loadEngines();
  }
  function setTrack(t) {
    if (!TRACKS[t] || t === S.track) return;
    const onWizard = current().wizard;
    S.track = t;
    S.i = onWizard ? slides().length - 1 : 0;
    store.set(POS_KEY, { track: S.track, i: S.i });
    render(true);
  }
  function openWizard() { go(slides().length - 1); }

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
    if (ds.track) return setTrack(ds.track);
    if (ds.wizard) return openWizard();
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

  function onInput(ev) {
    const t = ev.target, d = S.w.d;
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

  function visible() { return S.el && S.el.isConnected && S.el.offsetParent !== null; }
  function onKey(ev) {
    if (!visible() || ev.defaultPrevented || ev.altKey || ev.ctrlKey || ev.metaKey) return;
    const t = ev.target;
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
    if (document.querySelector("dialog[open]")) return;
    if (ev.key === "ArrowRight") { ev.preventDefault(); go(S.i + 1); }
    else if (ev.key === "ArrowLeft") { ev.preventDefault(); go(S.i - 1); }
  }
  let touch = null;
  function onTouchStart(ev) {
    if (ev.touches.length !== 1 || ev.target.closest("input,textarea,pre,.intro-tablewrap,.intro-code")) { touch = null; return; }
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
      if (pos && TRACKS[pos.track]) { S.track = pos.track; S.i = Math.max(0, Math.min(TRACKS[pos.track].slides.length - 1, pos.i | 0)); }
      el.addEventListener("click", onClick);
      el.addEventListener("input", onInput);
      el.addEventListener("touchstart", onTouchStart, { passive: true });
      el.addEventListener("touchend", onTouchEnd, { passive: true });
      if (!S.keyBound) { document.addEventListener("keydown", onKey); S.keyBound = true; }
      render();
      if (current().wizard) loadEngines();
    },
    show(ctx) {
      if (ctx) S.ctx = ctx;
      render();
      if (current().wizard) loadEngines();
    }
  };
})();
