//! The MCP tool catalogue (MCP/GraphRAG hardening, MG-1): one `ToolDoc` per
//! tool, rendered into the `tools/list` entry. Each description has six
//! labelled parts in this order, a contract the guard in mcp.rs reads:
//! `Use:`, `Arguments:` (generated from the input schema, so the two cannot
//! drift), `Example:` (one complete arguments object), `Returns:`, `Errors:`,
//! `Limits:`. Every input property carries a description and, where one
//! exists, a JSON Schema `default`; every tool declares an `outputSchema`
//! (its result is always an object, sent as `structuredContent`).

use serde_json::{Value, json};

pub struct ToolDoc {
    pub name: &'static str,
    pub title: &'static str,
    pub use_: &'static str,
    pub example: Value,
    pub returns: &'static str,
    pub errors: String,
    pub limits: &'static str,
    /// input properties, in the order the Arguments text lists them
    pub props: Vec<(&'static str, Value)>,
    pub required: &'static [&'static str],
    pub output: Value,
    pub read_only: bool,
}

/// The six labels, in order.
pub const LABELS: [&str; 6] = ["Use:", "Arguments:", "Example:", "Returns:", "Errors:", "Limits:"];

/// The error families every project tool can return (after MG-16).
const COMMON_ERRORS: &str = "403 \"forbidden: '<you>' is not a member of project '<p>'\" — ask an admin for a member edge (or use a project you serve); \
500 \"internal error (error_id e-…)\" — retry once, then quote the error_id to a human; \
\"no project: pass `project` or send the X-Iter-Project header\".";

fn type_text(s: &Value) -> String {
    if let Some(e) = s.get("enum").and_then(|e| e.as_array()) {
        return format!("one of {}", e.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" | "));
    }
    match s["type"].as_str().unwrap_or("any") {
        "array" => match s["items"]["type"].as_str() {
            Some(t) => format!("list of {t}"),
            None => "list".into(),
        },
        t => t.to_string(),
    }
}

impl ToolDoc {
    /// The `Arguments:` text, from the schema.
    pub fn arguments(&self) -> String {
        if self.props.is_empty() {
            return "none.".into();
        }
        self.props
            .iter()
            .map(|(k, s)| {
                let need = if self.required.contains(k) {
                    "required".to_string()
                } else if let Some(d) = s.get("default") {
                    format!("default {d}")
                } else if s["description"].as_str().unwrap_or("").contains("default") {
                    "optional".to_string()
                } else {
                    "no default".to_string()
                };
                format!("{k} ({}, {need}): {}", type_text(s), s["description"].as_str().unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn description(&self) -> String {
        let limits = if PEOPLE_ONLY.contains(&self.name) { format!("{}{PEOPLE}", self.limits) } else { self.limits.to_string() };
        let parts = [self.use_.to_string(), self.arguments(), self.example.to_string(), self.returns.to_string(), self.errors.clone(), limits];
        LABELS.iter().zip(parts).map(|(l, p)| format!("{l} {p}")).collect::<Vec<_>>().join("\n")
    }

    pub fn render(&self) -> Value {
        let props: serde_json::Map<String, Value> = self.props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        json!({
            "name": self.name, "title": self.title, "description": self.description(),
            "inputSchema": {"type": "object", "properties": props, "required": self.required},
            "outputSchema": self.output,
            "annotations": {"readOnlyHint": self.read_only, "destructiveHint": DESTRUCTIVE.contains(&self.name), "idempotentHint": self.read_only, "openWorldHint": false},
        })
    }
}

// ---------- schema helpers ----------

fn string(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn string_d(desc: &str, default: &str) -> Value {
    json!({"type": "string", "description": desc, "default": default})
}
fn integer(desc: &str, default: Option<i64>, min: i64, max: i64) -> Value {
    let mut v = json!({"type": "integer", "description": desc, "minimum": min, "maximum": max});
    if let Some(d) = default {
        v["default"] = json!(d);
    }
    v
}
fn boolean(desc: &str, default: bool) -> Value {
    json!({"type": "boolean", "description": desc, "default": default})
}
fn strings(desc: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": desc})
}
fn object(desc: &str) -> Value {
    json!({"type": "object", "description": desc})
}
fn one_of(desc: &str, values: &[&str], default: Option<&str>) -> Value {
    let mut v = json!({"type": "string", "enum": values, "description": desc});
    if let Some(d) = default {
        v["default"] = json!(d);
    }
    v
}
fn project() -> (&'static str, Value) {
    ("project", string("project id or name; default: the X-Iter-Project header"))
}
fn workid() -> (&'static str, Value) {
    ("workid", string("the calling work item (full id or a unique suffix); default: the X-Iter-Workid header"))
}

/// An output schema: an object with these properties, all required.
fn out(props: &[(&str, Value)]) -> Value {
    let p: serde_json::Map<String, Value> = props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    json!({"type": "object", "properties": p, "required": props.iter().map(|x| x.0).collect::<Vec<_>>()})
}
fn t(ty: &str) -> Value {
    json!({"type": ty})
}
fn opt(ty: &str) -> Value {
    json!({"type": [ty, "null"]})
}
fn list(items: Value) -> Value {
    json!({"type": "array", "items": items})
}
fn any() -> Value {
    json!({})
}
fn ok_message() -> Value {
    out(&[("ok", t("boolean")), ("message", t("string"))])
}
/// One work item in a list (wiquery::project_item).
fn brief() -> Value {
    out(&[("id", t("string")), ("name", opt("string")), ("state", opt("string")), ("agent", opt("string")), ("priority", opt("integer")),
          ("tags", list(t("string"))), ("blockedby", json!({"type": ["array", "null"]})), ("received", opt("string"))])
}

// ---------- example ids (placeholders the examples guard maps to seeded ones) ----------

/// the work items the examples name (by their last 12 characters)
#[cfg(test)]
pub const EX_ITEM: &str = "3f2a9c1e-5b7d-4e8a-9c0f-6f1c2a9b04de";
#[cfg(test)]
pub const EX_ITEM2: &str = "8d4e2b6a-1c3f-4a5e-b7d9-0e2f4a6c8b1d";
/// the people-only tools' items: a question, a failed item, a duplicate, one to delete
#[cfg(test)]
pub const EX_QUESTION: &str = "c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f";
#[cfg(test)]
pub const EX_FAILED: &str = "b2c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e";
#[cfg(test)]
pub const EX_DUP: &str = "d4e5f6a7-b8c9-4d0e-8f1a-2b3c4d5e6f70";
#[cfg(test)]
pub const EX_OLD: &str = "e5f6a7b8-c9d0-4e1f-9a2b-3c4d5e6f7a81";
pub const EX_NODE: &str = "dd366398-147d-450e-a792-59ef7ae020bc";
pub const EX_NODE2: &str = "f0ecfa98-f3cd-43f7-888b-64b9d3b53d81";
pub const EX_USECASE: &str = "5f31ed6e-34c6-4a3f-a37e-5e04d0a6b44d";
pub const EX_REQFILE: &str = "090b870c-dea5-4b5b-8ec1-74ce1dbf6d15";
pub const EX_REQ: &str = "ef4d98e0-e24d-4524-a1ac-14bc99635a4e";
pub const EX_TESTNODE: &str = "a3c5e7f9-2b4d-4f6a-8c0e-1d3f5a7b9c2e";
pub const EX_DOC: &str = "f3aa54129150e49d1b7261f2fc71ef63d";
pub const EX_NODE_PATH: &str = "{topdir}/src/billing/billing.code.iter.md";

// ---------- the catalogue ----------

/// Every tool, in `tools/list` order.
pub fn docs() -> Vec<ToolDoc> {
    vec![
        status(), workitem_list(), workitem_get(), workitem_details(), workitem_create(), workitem_ask(), workitem_reject(),
        workitem_wait(), workitem_doc(), workitem_block(), capability(), locks_list(),
        graph_stats(), graph_lookup(), graph_node(), graph_neighbors(), graph_owner(), graph_usecase(),
        graph_node_create(), graph_node_update(), graph_edge_add(), graph_edge_remove(),
        req_create(), req_update(), req_move(), req_delete(), testlogs(), settings_graph(),
        rag_search(), rag_status(), rag_docs(), rag_doc(), rag_link_document(), rag_add_document(),
        // people only (PEOPLE_ONLY)
        whoami(), workitem_answer(), workitem_set_state(), workitem_stop(), workitem_priority(), workitem_reopen(),
        workitem_edit(), workitem_clone(), workitem_duplicate_of(), workitem_explain(), workitem_delete(),
        rag_delete_document(), rag_resummarize_document(),
    ]
}

/// The tools for a person's token only (2026-10-21): the webui's work queue
/// controls and GraphRAG document upkeep, for a person or an agent acting as
/// one (e.g. a claude.ai connector signed in as a user). Engine sessions —
/// iter's own agents, who see only the item they work on — neither list nor
/// call them.
pub const PEOPLE_ONLY: &[&str] = &[
    "whoami", "workitem_answer", "workitem_set_state", "workitem_stop", "workitem_priority", "workitem_reopen",
    "workitem_edit", "workitem_clone", "workitem_duplicate_of", "workitem_explain", "workitem_delete",
    "rag_delete_document", "rag_resummarize_document",
];

/// Tools that remove something for good (annotation destructiveHint).
pub const DESTRUCTIVE: &[&str] = &["workitem_delete", "rag_delete_document", "req_delete", "graph_edge_remove"];

/// Every rendered `tools/list` entry (built once).
pub fn tools() -> &'static [Value] {
    static TOOLS: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    TOOLS.get_or_init(|| docs().iter().map(ToolDoc::render).collect())
}

/// The entries one caller sees: everything for a person's token, the
/// people-only tools left out for an engine session.
pub fn tools_for(person: bool) -> Vec<Value> {
    tools().iter().filter(|t| person || !PEOPLE_ONLY.iter().any(|n| t["name"] == *n)).cloned().collect()
}

// ----- work queue -----

fn status() -> ToolDoc {
    ToolDoc {
        name: "status", title: "Project status", read_only: true,
        use_: "see what is open right now (in progress, queued, question, paused, parked, scheduled), in `iter status` order, plus wait-for deadlocks and stale lock rows; use workitem_list to search or page through every item, closed ones included.",
        props: vec![project()], required: &[],
        example: json!({"project": "pdy-dev"}),
        returns: "{project: string, open: [{id, name, state, agent, priority, tags: [string], blockedby: [string], received}], open_count: integer, deadlocks: [{members, edges, text}], stale_lock_rows: integer}",
        errors: COMMON_ERRORS.into(),
        limits: "open items only, every one (no paging); ordered by state then priority; read at call time.",
        output: out(&[("project", t("string")), ("open", list(brief())), ("open_count", t("integer")), ("deadlocks", t("array")), ("stale_lock_rows", t("integer"))]),
    }
}

fn workitem_list() -> ToolDoc {
    ToolDoc {
        name: "workitem_list", title: "List work items", read_only: true,
        use_: "page through a project's work items — every one, closed included — with any filter and sort; use workitem_get for one item's full record and status for just the open queue.",
        props: vec![
            project(),
            ("filter", json!({"type": "array", "items": {"type": "object"}, "description":
                "clauses ANDed: {field, op, value} with a dotted field (state, priority, ts.receive, tags.text, sched.kind, any stored field) and op eq | ne | in | nin | gt | gte | lt | lte | contains | prefix | exists | empty; {any: [clauses]} ORs. A missing or null field reads as false/empty: it equals false, \"\", [] and null, is empty, never matches gt/gte/lt/lte/contains/prefix. A path through a list matches when any element does (ne/nin: when none does). No default"})),
            ("sort", json!({"type": "array", "items": {"type": "object"}, "default": [{"field": "ts.receive", "dir": "desc"}], "description":
                "[{field, dir: asc|desc}] on any dotted field; id ascending is always added last. Missing sorts lowest; types order null < boolean < number < string < list < object; a list sorts by its first element"})),
            ("limit", integer("items per page", Some(50), 1, 500)),
            ("cursor", string("next_cursor of the previous page of the SAME query (same filter and sort); no default")),
            ("fields", strings("extra dotted paths added to each item (e.g. ts.start, requestedby, lasterror), or [\"*\"] for the whole record; no default")),
            ("state", string("shortcut: filter state eq this; no default")),
            ("tag", string("shortcut: filter tags.text eq this; no default")),
            ("text", string("shortcut: filter name contains this (case-insensitive); no default")),
        ],
        required: &[],
        example: json!({"project": "pdy-dev", "filter": [{"field": "state", "op": "in", "value": ["queued", "question"]},
                        {"field": "ts.receive", "op": "gte", "value": "2026-10-01"}], "sort": [{"field": "priority", "dir": "asc"}], "limit": 100}),
        returns: "{total: integer (items matching the filter, counted at this call), items: [{id, name, state, agent, priority, tags: [string], blockedby: [string], received: string (ts.receive), …fields}], next_cursor: string|null (null on the last page), sort: [{field, dir}], unknown_fields: [string] (filter/sort paths no item has: usually a typo)}",
        errors: errs("400 \"cursor belongs to another query\" — drop the cursor and start again from page one; 400 \"unknown op X\" / \"a filter clause needs a field\" — fix the clause; 403 not a member of the project; 500 internal error (error_id …)."),
        limits: "1–500 items per page; page with next_cursor until it is null (an item created meanwhile neither repeats nor shifts a page); total is counted at call time and can grow between pages.",
        output: out(&[("total", t("integer")), ("items", list(brief())), ("next_cursor", opt("string")),
                      ("sort", list(out(&[("field", t("string")), ("dir", t("string"))]))), ("unknown_fields", list(t("string")))]),
    }
}

fn workitem_get() -> ToolDoc {
    ToolDoc {
        name: "workitem_get", title: "Get a work item", read_only: true,
        use_: "one work item's full stored record (state, agent, priority, tags, lockdirs, blockedby, ts, requestedby, createdby, lasterror…); use workitem_details for its history rows.",
        props: vec![project(), ("id", string("the full id or a unique suffix (the webui shows the last 12 characters)"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "6f1c2a9b04de"}),
        returns: "the record: {id: string, project: string, name: string, state: string, agent: string, priority: integer, version: integer, tags: [{text, color}], lockdirs: [string], blockedby: [string], ts: {receive, start, complete}, requestedby: string, createdby: string, …}",
        errors: errs("\"no work item in '<p>' matches '<id>'\" — check the id; \"'<id>' is ambiguous (N matches)\" — give more of it; \"work item id: an id is empty\" (or holds a control character, or is over 200 characters); 404 not found; "),
        limits: "read at call time; a suffix must match exactly one item of the project.",
        output: out(&[("id", t("string")), ("state", t("string"))]),
    }
}

fn workitem_details() -> ToolDoc {
    ToolDoc {
        name: "workitem_details", title: "Work item history", read_only: true,
        use_: "a work item's detail rows in order: the request, runs, doc notes, questions and answers, reviews, spend; use workitem_get for the record itself.",
        props: vec![project(), ("id", string("the full id or a unique suffix"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "6f1c2a9b04de"}),
        returns: "{id: string (the full id), details: [{id, order: integer, key: string (request|doc|question|run|…), valuetype: string, value, by: string, by_workid?: string, ts: string}]}",
        errors: errs("\"no work item in '<p>' matches '<id>'\"; \"'<id>' is ambiguous\"; "),
        limits: "every row, oldest first; a long run log can be large.",
        output: out(&[("id", t("string")), ("details", list(out(&[("key", t("string")), ("order", t("integer")), ("by", t("string"))])))]),
    }
}

fn workitem_create() -> ToolDoc {
    ToolDoc {
        name: "workitem_create", title: "Create a work item (iter add)", read_only: false,
        use_: "file new work (or, with `question`, a question for a human). Inside a work item the new item becomes its child: it inherits priority and use-case tags and counts for the close gate. Who filed it is set by the server from your token, never from the arguments.",
        props: vec![
            project(), workid(),
            ("title", string("one line naming the work")),
            ("agent", string_d("code | plan | test | usecase | … (the agent that runs it)", "code")),
            ("request", string("the instructions: what to do and how to know it is done; required unless `question` is given; no default")),
            ("codepaths", strings("lock scope: {topdir}/… or repo-relative paths; no default")),
            ("priority", integer("0–99, lower = sooner; ignored inside an agent (children inherit); no default (the server picks the lowest free number in the band)", None, 0, 99)),
            ("depends_on", strings("ids or unique suffixes this item waits for (deep: their children too); no default")),
            ("depends_on_shallow", boolean("wait only for the named items, not their children", false)),
            ("context", strings("files or notes the agent reads first; no default")),
            ("model", string("model override for the run; no default (the agent's)")),
            ("tags", strings("tag texts, optionally text:#color; no default")),
            ("usecase", string("the use case this serves (becomes the usecase:<name> tag); no default")),
            ("question", string("file the item AS a question for a human (the six-part shape); it queues for work once answered; no default")),
        ],
        required: &["title"],
        example: json!({"project": "pdy-dev", "title": "Retry the clearing export on timeout", "agent": "code",
                        "request": "Make the clearing export retry twice on a gateway timeout. Done when the new unit test passes.",
                        "codepaths": ["src/clearing/"], "tags": ["clearing"]}),
        returns: "{id: string, state: string, agent: string, priority: integer, requestedby: string (user:<you> | agent:<agent>@<engine> | engine:<engine>), createdby: string (the parent id, or the same principal), already_open: boolean, warnings: [string], message: string}",
        errors: errs("\"a title is required\"; \"an item needs a request … or a question\"; \"an item cannot depend on the item that creates it\"; 400 \"refused: the request is a placeholder\"; 400 \"dependency cycle: …\" — drop the dependency named; 403 \"schedules are users-only\"; "),
        limits: "requestedby/createdby are set by the server from the token (a sent value is ignored and named in warnings); a repeat of an open item with the same check and container returns that item with already_open: true instead of filing a second one.",
        output: out(&[("id", t("string")), ("state", any()), ("agent", any()), ("priority", any()), ("requestedby", any()), ("createdby", any()),
                      ("already_open", t("boolean")), ("warnings", t("array")), ("message", t("string"))]),
    }
}

fn workitem_ask() -> ToolDoc {
    ToolDoc {
        name: "workitem_ask", title: "Ask the human (iter ask)", read_only: false,
        use_: "record a question on the calling work item; it parks in `question` when this turn ends and queues again once a human answers. Finish your turn after calling it.",
        props: vec![project(), workid(), ("question", string("the question, in the six-part shape your instructions give"))],
        required: &["question"],
        example: json!({"project": "pdy-dev", "workid": "6f1c2a9b04de", "question": "Should the export retry on HTTP 502 as well as 504?"}),
        returns: "{ok: boolean, message: string}",
        errors: errs("\"the question is empty\"; \"no calling work item: pass workid or send X-Iter-Workid\"; \"no work item in '<p>' matches '<id>'\"; 409 conflict — the item changed meanwhile, call again; "),
        limits: "acts on the calling item only; the question shows in the webui at once.",
        output: ok_message(),
    }
}

fn workitem_reject() -> ToolDoc {
    ToolDoc {
        name: "workitem_reject", title: "Reject the calling item (iter reject)", read_only: false,
        use_: "the calling work item is invalid (wrong, impossible, already done): it parks for human review with the reason recorded as \"rejected by <you>: <reason>\"; no retry is burned.",
        props: vec![project(), workid(), ("reason", string("why the item is invalid"))],
        required: &["reason"],
        example: json!({"project": "pdy-dev", "workid": "6f1c2a9b04de", "reason": "The clearing export was removed in July; nothing to retry."}),
        returns: "{ok: boolean, message: string}",
        errors: errs("\"a reason is required\"; \"no calling work item: pass workid or send X-Iter-Workid\"; \"no work item in '<p>' matches '<id>'\"; 403 \"closed workitem is immutable\" — it is already closed; "),
        limits: "the actor named is the server's principal for your token, not the item's agent.",
        output: ok_message(),
    }
}

fn workitem_wait() -> ToolDoc {
    ToolDoc {
        name: "workitem_wait", title: "Wait on other items (iter wait)", read_only: false,
        use_: "declare that the calling item cannot finish until other items land: links them as deep dependencies; the close gate then queues this item behind them.",
        props: vec![project(), workid(), ("on", strings("ids or unique suffixes of the items to wait for")), ("reason", string("why; recorded on the item; no default"))],
        required: &["on"],
        example: json!({"project": "pdy-dev", "workid": "6f1c2a9b04de", "on": ["0e2f4a6c8b1d"], "reason": "needs the new gateway client first"}),
        returns: "{ok: boolean, added: [string] (full ids newly linked), message: string}",
        errors: errs("\"`on` names no work item\"; \"an item cannot wait on itself\"; \"no work item in '<p>' matches '<id>'\"; 400 \"dependency cycle: …\" — the items already wait on this one; 409 conflict — call again; "),
        limits: "adds links only, never removes; finish your turn listing what still waits as NOT DONE lines.",
        output: out(&[("ok", t("boolean")), ("added", list(t("string"))), ("message", t("string"))]),
    }
}

fn workitem_doc() -> ToolDoc {
    ToolDoc {
        name: "workitem_doc", title: "Append a note (iter doc)", read_only: false,
        use_: "append a `doc` note to a work item — the calling one by default, or any item by `id`; allowed on closed items too.",
        props: vec![project(), workid(), ("id", string("target item, full id or unique suffix; default: the calling item")), ("text", string("the note (markdown)"))],
        required: &["text"],
        example: json!({"project": "pdy-dev", "id": "6f1c2a9b04de", "text": "Gateway timeouts were 504s only, per the access log of 2026-10-20."}),
        returns: "{ok: boolean, id: string (the full id written to), order: integer}",
        errors: errs("\"doc text is empty\"; \"no calling work item: pass workid or send X-Iter-Workid\"; \"no work item in '<p>' matches '<id>'\"; "),
        limits: "notes are appended, never edited; the row's `by` is your token's subject.",
        output: out(&[("ok", t("boolean")), ("id", t("string")), ("order", opt("integer"))]),
    }
}

fn workitem_block() -> ToolDoc {
    ToolDoc {
        name: "workitem_block", title: "Block on the cluster restart (iter block)", read_only: false,
        use_: "park the calling item until the nightly cluster restart finishes, giving its attempt back; only for work that needs the cluster down or rebuilt.",
        props: vec![project(), workid(), ("reason", string_d("why the cluster is needed", "the cluster is required for this work"))],
        required: &[],
        example: json!({"project": "pdy-dev", "workid": "6f1c2a9b04de", "reason": "the schema migration needs the restart window"}),
        returns: "{ok: boolean, attempt_before: integer, attempt_after: integer, message: string}",
        errors: errs("\"refused: tonight's cluster rebuild was skipped …\" — do the work now or ask; \"no calling work item: pass workid or send X-Iter-Workid\"; 409 conflict — call again; "),
        limits: "the engine requeues the item once the cluster is healthy; finish your turn after calling it.",
        output: out(&[("ok", t("boolean")), ("attempt_before", t("integer")), ("attempt_after", t("integer")), ("message", t("string"))]),
    }
}

fn capability() -> ToolDoc {
    ToolDoc {
        name: "capability", title: "Capabilities (iter capability)", read_only: true,
        use_: "read the capability documents (how-tos agents share: AWS access, deploys…): without `name` the list, with `name` one document's full text.",
        props: vec![("name", string("a capability's id or name; no default (the list)"))],
        required: &[],
        example: json!({"name": "aws"}),
        returns: "without name: {capabilities: [{name, title, desc}]}; with name: {name: string, title: string, body: string}",
        errors: errs("\"no capability named '<name>'\" — list them first; 500 internal error (error_id …)."),
        limits: "global (no project); read at call time.",
        output: json!({"type": "object", "properties": {"capabilities": {"type": "array"}, "name": {}, "title": {}, "body": {}}, "required": []}),
    }
}

fn locks_list() -> ToolDoc {
    ToolDoc {
        name: "locks_list", title: "Locks", read_only: true,
        use_: "the project's live lock rows: which path each work item holds, until when, and whether the holder's lease is still live.",
        props: vec![project()],
        required: &[],
        example: json!({"project": "pdy-dev"}),
        returns: "{locks: [{path: string, kind: string (lock|reserve), workid: string, engine: string, acquired: string, expires: string, lease: string, holder_state: string, holder_lease_live: boolean}]}",
        errors: COMMON_ERRORS.into(),
        limits: "unexpired rows only; read at call time (locks change every tick).",
        output: out(&[("locks", list(out(&[("path", t("string")), ("workid", t("string")), ("holder_state", t("string")), ("holder_lease_live", t("boolean"))])))]),
    }
}

// ----- the map -----

fn graph_stats() -> ToolDoc {
    ToolDoc {
        name: "graph_stats", title: "Graph stats", read_only: true,
        use_: "counts of the project map: nodes by type, edges by kind, file sync states, nodes unreachable from the project node, red tests; a cheap first look before walking the map.",
        props: vec![project()],
        required: &[],
        example: json!({"project": "pdy-dev"}),
        returns: "{project: string, nodes: integer, files: integer, reqs: integer, edges: integer, by_nodetype: {type: count}, by_kind: {kind: count}, file_states: {state: count}, pending_files: integer, designed: integer, project_node: string|null, reachable_from_project: integer, unreachable: [{id, nodetype, name, path}], red_tests: [{id, name, path}], backend: string, sync_paused: boolean}",
        errors: COMMON_ERRORS.into(),
        limits: "the map as iter_data holds it (an engine's unsynced edits are not in it yet).",
        output: out(&[("project", t("string")), ("nodes", t("integer")), ("files", t("integer")), ("reqs", t("integer")), ("edges", t("integer")),
                      ("by_nodetype", t("object")), ("by_kind", t("object")), ("unreachable", t("array")), ("red_tests", t("array"))]),
    }
}

fn graph_lookup() -> ToolDoc {
    ToolDoc {
        name: "graph_lookup", title: "Find a map node", read_only: true,
        use_: "find map nodes by file `path` ({topdir}/…/x.code.iter.md) or by `name` (optionally of one nodetype); use graph_owner to find who owns a source path.",
        props: vec![project(), ("path", string("a node file path; no default")), ("name", string("a node name (exact, then case-insensitive); no default")),
                    ("nodetype", string("project | code | test | bizreq | techreq | philosophy | usecase | actor; no default"))],
        required: &[],
        example: json!({"project": "pdy-dev", "name": "billing", "nodetype": "code"}),
        returns: "{matched_by: string (path|name|\"\"), matches: [node: {id, name, nodetype, level, path, desc, …}]}",
        errors: COMMON_ERRORS.into(),
        limits: "give path or name; no match is an empty list, not an error.",
        output: out(&[("matched_by", t("string")), ("matches", t("array"))]),
    }
}

fn graph_node() -> ToolDoc {
    ToolDoc {
        name: "graph_node", title: "Get a map node", read_only: true,
        use_: "one node of the map by id, with its file text and its edges in and out; use graph_neighbors to walk further.",
        props: vec![project(), ("id", string("the node id (graph_lookup finds it)"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_NODE}),
        returns: "{id: string, name: string, nodetype: string, path: string, desc: string, body: string, children: {…}, node_version: integer, file_state: string, text: string, edges_out: [{kind, to}], edges_in: [{kind, from}]}",
        errors: errs("\"id is required\"; 404 \"no node <id> in <p>\"; "),
        limits: "requirement sections (type req) return the section instead of a file text.",
        output: out(&[("id", t("string")), ("edges_out", t("array")), ("edges_in", t("array"))]),
    }
}

fn graph_neighbors() -> ToolDoc {
    ToolDoc {
        name: "graph_neighbors", title: "Map neighbours", read_only: true,
        use_: "nodes linked to a node, to a depth, following links out, in, or both.",
        props: vec![project(), ("id", string("the start node id")), ("depth", integer("hops", Some(1), 1, 6)),
                    ("direction", one_of("which links to follow", &["out", "in", "any"], Some("any")))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_NODE, "depth": 2, "direction": "out"}),
        returns: "{start: string, direction: string, depth: integer, neighbors: [{vertex: node, via: string (edge kind), from: string, to: string, depth: integer}]}",
        errors: errs("\"id is required\"; 404 \"no node <id> in <p>\"; "),
        limits: "breadth-first; each node once, by the first edge that reached it.",
        output: out(&[("start", t("string")), ("direction", t("string")), ("depth", t("integer")), ("neighbors", t("array"))]),
    }
}

fn graph_owner() -> ToolDoc {
    ToolDoc {
        name: "graph_owner", title: "Owner of a path", read_only: true,
        use_: "which code nodes own a source path (their code directories contain it), deepest first: where to read before changing a file.",
        props: vec![project(), ("path", string("{topdir}/… or repo-relative source path"))],
        required: &["path"],
        example: json!({"project": "pdy-dev", "path": "{topdir}/src/billing/invoice.rs"}),
        returns: "{owners: [node: {id, name, nodetype, level, path, …}]}",
        errors: errs("\"path is required\"; "),
        limits: "glob ownership as the nodes declare it; an unowned path gives an empty list.",
        output: out(&[("owners", t("array"))]),
    }
}

fn graph_usecase() -> ToolDoc {
    ToolDoc {
        name: "graph_usecase", title: "A use case's parts", read_only: true,
        use_: "a use case (id or name) with the code nodes it uses, everything they own, and the actors that drive it.",
        props: vec![project(), ("ucid", string("the use case's node id or name"))],
        required: &["ucid"],
        example: json!({"project": "pdy-dev", "ucid": EX_USECASE}),
        returns: "{usecase: node, tops: [node], actors: [node], nodes: [node]}",
        errors: errs("\"ucid is required\"; 404 no such use case; "),
        limits: "follows uses/codenodes edges as stored.",
        output: out(&[("usecase", t("object")), ("tops", t("array")), ("actors", t("array")), ("nodes", t("array"))]),
    }
}

fn graph_node_create() -> ToolDoc {
    ToolDoc {
        name: "graph_node_create", title: "Create a graph node", read_only: false,
        use_: "create a map node (an engine writes its file): nodetype project|code|test|bizreq|techreq|philosophy|usecase|actor; hang it under a parent with attach_to.",
        props: vec![
            project(),
            ("nodetype", string("project | code | test | bizreq | techreq | philosophy | usecase | actor")),
            ("name", string("the node's name")),
            ("level", string("code nodes: context | container | component | connection; no default")),
            ("desc", string("~100 words: enough to decide whether to read the whole node; no default")),
            ("body", string("markdown body (a bizreq/techreq: the one requirement + rationale); no default")),
            ("attach_to", string("a parent node id; no default (root contexts and reqs go under the project node)")),
            ("attach_kind", string("codenodes | tests | reqs | uses | drives | touches | supplies | connects | calls | pulls | pushes | same_as; no default (inferred)")),
            ("front", object("type-specific keys (status, owner, flowmap, connects, …); no default")),
            ("children", object("codedirs / codenodes / tests / reqs path lists; no default")),
        ],
        required: &["nodetype", "name"],
        example: json!({"project": "pdy-dev", "nodetype": "code", "level": "component", "name": "invoice-export", "desc": "Exports settled invoices to the clearing house.", "attach_to": EX_NODE}),
        returns: "{node: {id, name, nodetype, level, path, …}, touched: [string] (ids changed)}",
        errors: errs("400 \"nodetype must be one of …\" / \"a node needs a name\" / a bad level; 409 \"exists\" (with the existing id) — update that node instead; 409 \"the project has its project node already\"; "),
        limits: "visible in the graph at once; the file appears in the checkout after an engine's next sync (about a minute).",
        output: out(&[("node", t("object")), ("touched", t("array"))]),
    }
}

fn graph_node_update() -> ToolDoc {
    ToolDoc {
        name: "graph_node_update", title: "Update a graph node", read_only: false,
        use_: "change a node's name, desc, body, teststate, level, front keys (null removes one) or children lists (a given list replaces the old one).",
        props: vec![
            project(), ("id", string("the node id")),
            ("expect_version", integer("refuse the write when the node's node_version is no longer this; no default", None, 0, i64::MAX)),
            ("name", string("new name (the file is not moved); no default")), ("desc", string("no default")), ("body", string("no default")),
            ("teststate", one_of("test participation; no default", &["inherit", "include", "omit", "block"], None)),
            ("level", string("no default")), ("front", object("keys to set; null removes; no default")), ("children", object("lists to replace; no default")),
        ],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_NODE, "desc": "Exports settled invoices to the clearing house, retrying gateway timeouts."}),
        returns: "the node after the edit: {id, name, nodetype, path, node_version: integer, …, moved?: [string], nodes?: [node]}",
        errors: errs("\"id is required\"; 404 \"no node <id> in <p>\"; 409 conflict {current} — re-read and retry with its node_version; 400 bad value; "),
        limits: "reaches search after the engine writes the file and its next sync (about a minute).",
        output: out(&[("id", t("string"))]),
    }
}

fn graph_edge_add() -> ToolDoc {
    ToolDoc {
        name: "graph_edge_add", title: "Add a graph edge", read_only: false,
        use_: "add an edge between two nodes; it is written into the owning node's file. kinds: codenodes, tests, reqs, uses (use case → code), drives (actor → use case), touches (actor → code), supplies (code → connection), connects (connection → code), calls / pulls / pushes (code → code), same_as (code → its master copy).",
        props: vec![project(), ("from", string("source node id")), ("to", string("target node id")), ("kind", string("the edge kind (see Use)"))],
        required: &["from", "to", "kind"],
        example: json!({"project": "pdy-dev", "from": EX_NODE, "to": EX_NODE2, "kind": "calls"}),
        returns: "{edge: {from, kind, to, protocol}, touched: [string], nodes: [node]}",
        errors: errs("400 an unknown kind, or one that does not join these node types; 404 no such node; "),
        limits: "an edge that exists already is a no-op.",
        output: out(&[("edge", t("object")), ("touched", t("array"))]),
    }
}

fn graph_edge_remove() -> ToolDoc {
    ToolDoc {
        name: "graph_edge_remove", title: "Remove a graph edge", read_only: false,
        use_: "remove an edge (from, to, kind) with a reason recorded in the commit.",
        props: vec![project(), ("from", string("source node id")), ("to", string("target node id")), ("kind", string("the edge kind")), ("reason", string("why"))],
        required: &["from", "to", "kind", "reason"],
        example: json!({"project": "pdy-dev", "from": EX_NODE, "to": EX_NODE2, "kind": "calls", "reason": "the export no longer calls the gateway directly"}),
        returns: "{removed: {from, kind, to}, touched: [string], nodes: [node]}",
        errors: errs("409 refused when only a glob pattern of the owning node matches the target — change that pattern with graph_node_update; 404 no such edge; "),
        limits: "the file changes after an engine's next sync.",
        output: out(&[("removed", t("object")), ("touched", t("array"))]),
    }
}

// ----- requirements -----

fn req_create() -> ToolDoc {
    ToolDoc {
        name: "req_create", title: "Add a requirement", read_only: false,
        use_: "add one requirement (a `## ` section) to a requirement file: name the file, or the code/project node it belongs to plus `type` (that node's file is created when it has none).",
        props: vec![
            project(),
            ("file", string("a bizreq/techreq node id; no default")),
            ("node", string("the code or project node the requirement belongs to (with type); no default")),
            ("type", one_of("with node: which file; no default", &["bizreq", "techreq"], None)),
            ("key", string("e.g. PDY-TECH-034; no default")),
            ("title", string("the section title")),
            ("text", string("markdown; ### or lower for sub-headings; no default")),
            ("status", one_of("no default (draft)", &["draft", "agreed", "done"], None)),
            ("after", string("a req id of that file to place it after; no default (last)")),
        ],
        required: &["title"],
        example: json!({"project": "pdy-dev", "file": EX_REQFILE, "key": "PDY-TECH-034", "title": "Exports retry gateway timeouts", "text": "A 504 from the gateway is retried twice.", "status": "draft"}),
        returns: "{req: {id, key, title, text, status, file}, file: node, touched: [string]}",
        errors: errs("400 neither file nor node+type given; 404 no such file/node; "),
        limits: "the file changes after an engine's next sync.",
        output: out(&[("req", any()), ("file", any()), ("touched", t("array"))]),
    }
}

fn req_update() -> ToolDoc {
    ToolDoc {
        name: "req_update", title: "Update a requirement", read_only: false,
        use_: "change a requirement's key, title, text or status; the file's other sections are untouched.",
        props: vec![project(), ("id", string("the requirement id")), ("key", string("no default")), ("title", string("no default")), ("text", string("no default")),
                    ("status", one_of("no default", &["draft", "agreed", "done"], None)),
                    ("expect_version", integer("the file node's node_version; refused when the file changed since; no default", None, 0, i64::MAX))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_REQ, "status": "agreed"}),
        returns: "{req: {id, key, title, text, status, file}, file: node, touched: [string]}",
        errors: errs("\"id is required\"; 404 \"no requirement <id>\"; 409 conflict {req, file} — re-read and retry; "),
        limits: "the file changes after an engine's next sync.",
        output: out(&[("req", any()), ("file", any()), ("touched", t("array"))]),
    }
}

fn req_move() -> ToolDoc {
    ToolDoc {
        name: "req_move", title: "Move a requirement", read_only: false,
        use_: "move a requirement to another requirement file of the project (same id), or reorder it within its file.",
        props: vec![project(), ("id", string("the requirement id")), ("to_file", string("the target bizreq/techreq node id")),
                    ("after", string("a req id of the target file; no default (last)"))],
        required: &["id", "to_file"],
        example: json!({"project": "pdy-dev", "id": EX_REQ, "to_file": EX_REQFILE}),
        returns: "{req, file, from_file, edge: {from, kind, to}, removed: {from, kind, to}, touched: [string]}",
        errors: errs("\"id is required\"; 404 \"no requirement <id>\"; 400 the target is not a requirement file; "),
        limits: "both files change after an engine's next sync.",
        output: out(&[("req", any()), ("file", any()), ("touched", t("array"))]),
    }
}

fn req_delete() -> ToolDoc {
    ToolDoc {
        name: "req_delete", title: "Delete a requirement", read_only: false,
        use_: "remove one requirement from its file, with a reason recorded in the commit.",
        props: vec![project(), ("id", string("the requirement id")), ("reason", string("why"))],
        required: &["id", "reason"],
        example: json!({"project": "pdy-dev", "id": EX_REQ, "reason": "superseded by PDY-TECH-035"}),
        returns: "{id: string, removed: boolean, file: node, touched: [string]}",
        errors: errs("\"id is required\"; 404 \"no requirement <id>\"; "),
        limits: "the file changes after an engine's next sync.",
        output: out(&[("id", t("string")), ("removed", t("boolean")), ("touched", t("array"))]),
    }
}

// ----- other -----

fn testlogs() -> ToolDoc {
    ToolDoc {
        name: "testlogs", title: "Test logs", read_only: true,
        use_: "the test results posted for the project's test nodes, newest first: outcome, bucket counts, failing details, engine, work item.",
        props: vec![project(), ("node", string("only this test node (id); no default (all)")), ("limit", integer("rows", Some(50), 1, 1000))],
        required: &[],
        example: json!({"project": "pdy-dev", "node": EX_TESTNODE, "limit": 10}),
        returns: "{logs: [{log_id, node, at: string, overall_success: boolean, normal: {total, pass, err}, longtail, failure, details: [..], engine, workid}]}",
        errors: COMMON_ERRORS.into(),
        limits: "1–1000 rows, newest first.",
        output: out(&[("logs", t("array"))]),
    }
}

fn settings_graph() -> ToolDoc {
    ToolDoc {
        name: "settings_graph", title: "Settings graph", read_only: true,
        use_: "the settings graph: nodes (iter_data, engines, projects, agents, tooling, users, accounts, providers, work item states) and edges (serves, bills, holds, member, runs, …) with their settings.",
        props: vec![],
        required: &[],
        example: json!({}),
        returns: "{nodes: [{id, type, name, settings, active, …}], edges: [{id, type, from, to, active, settings}]}",
        errors: errs("500 internal error (error_id …)."),
        limits: "admins see all of it; others the parts touching their projects; secrets are never shown.",
        output: out(&[("nodes", t("array")), ("edges", t("array"))]),
    }
}

// ----- GraphRAG -----

fn rag_search() -> ToolDoc {
    ToolDoc {
        name: "rag_search", title: "Search the project's documents", read_only: true,
        use_: "search the project's GraphRAG index (uploaded documents and *.iter.md node files) by meaning and keyword at once; each result is a FULL chunk with its chapter and document summaries and its place in the map. Search before reading files blind or asking a human.",
        props: vec![
            project(),
            ("query", string("a question or description in plain words")),
            ("k", integer("results", Some(8), 1, 50)),
            ("kinds", json!({"type": "array", "items": {"type": "string", "enum": ["file", "node", "guide"]}, "description": "file = uploads, node = *.iter.md files, guide = the iter user guide; no default (all)"})),
            ("include_guide", boolean("also search the iter user guide", true)),
            ("nodetypes", strings("node files of these types only (project, code, test, bizreq, techreq, philosophy, usecase, actor); no default")),
            ("doc", string("search inside one document (its id); no default")),
            ("mode", one_of("hybrid = keyword + raw text + summary rankings fused; keyword for exact names and codes", &["hybrid", "vector", "keyword"], Some("hybrid"))),
            ("vectors", one_of("which vectors the meaning rankings use", &["both", "raw", "summary"], Some("both"))),
            ("docs", boolean("also rank whole documents by their summaries", false)),
            ("graph", boolean("attach map neighbours to node hits", true)),
        ],
        required: &["query"],
        example: json!({"project": "pdy-dev", "query": "how are clearing exports retried after a gateway timeout?", "k": 5}),
        returns: "{query: string, k: integer, mode: string, method: string (exact|vector-index), ms: integer, model: string, candidates: {raw, summary, keyword}, results: [{id, doc, title, kind, path, nodetype, heading, text: string (the full chunk), summary, chapter_summary, doc_summary, score, similarity, ranks, graph?: {neighbors|linked_nodes}}], documents?: [{id, title, kind, path, summary, score}]}",
        errors: errs("400 \"query is required\"; 503 internal error (error_id …) — the embedding model is not loaded on the server; "),
        limits: "k ≤ 50. A document is in the index once searchable (rag_doc): an upload ~5–30 s after it is sent when an engine is live; summary search only once it is ready (minutes later). Node files are indexed by engines from the checkout after each map sync that changed something (at most every 60 s).",
        output: out(&[("query", t("string")), ("k", t("integer")), ("mode", t("string")), ("method", t("string")), ("results", t("array"))]),
    }
}

fn rag_status() -> ToolDoc {
    ToolDoc {
        name: "rag_status", title: "GraphRAG status", read_only: true,
        use_: "the health of the project's GraphRAG index: documents by kind and state, chunks by summary state, work waiting for engines, the engines serving the project and whether they are live, the change-sweep schedule.",
        props: vec![project()],
        required: &[],
        example: json!({"project": "pdy-dev"}),
        returns: "{project: string, docs: [{kind, state, n}], chunks: [{state, n}], waiting: integer, settings: {docs_dir, docs_gitignore, node_types, repo_globs}, model: {model, dim, loaded, stamp, dir? (admins)}, model_mismatch: integer, vector_index: {indexes, note}, engines: [{name, display, account, hold, last_seen, age_sec: integer|null, live: boolean}], schedule: object|null, accepted: [string]}",
        errors: COMMON_ERRORS.into(),
        limits: "an engine is live when it checked in within 3 × its tick + 5 s; with no live engine nothing new is indexed or summarised.",
        output: out(&[("project", t("string")), ("docs", t("array")), ("chunks", t("array")), ("waiting", t("integer")), ("model", t("object")),
                      ("engines", list(out(&[("name", t("string")), ("display", t("string")), ("account", t("string")), ("hold", t("string")),
                                              ("last_seen", t("string")), ("age_sec", opt("integer")), ("live", t("boolean"))])))]),
    }
}

fn rag_docs() -> ToolDoc {
    ToolDoc {
        name: "rag_docs", title: "GraphRAG documents", read_only: true,
        use_: "every document in the project's GraphRAG index, one row each (no text, no score): kind, title, path, state, whether it is searchable, chunk and chapter counts, its summary, and where its file sits on each engine's checkout.",
        props: vec![project(), ("kind", one_of("only this kind; no default (all)", &["file", "node"], None)), ("state", string("only this state (queued|ingesting|summarizing|rollup|ready|failed); no default"))],
        required: &[],
        example: json!({"project": "pdy-dev", "kind": "file"}),
        returns: "{documents: [{id, kind, title, path, state, searchable: boolean, chunks: integer, chapters, summarized: integer, linked: integer, summary, error, store?: {state, commit}, locations: [{engine, path, live}]}]}",
        errors: COMMON_ERRORS.into(),
        limits: "newest first, every document (no paging); searchable is true once chunks are stored and the state is summarizing, rollup or ready.",
        output: out(&[("documents", list(out(&[("id", t("string")), ("state", t("string")), ("searchable", t("boolean"))])))]),
    }
}

fn rag_doc() -> ToolDoc {
    ToolDoc {
        name: "rag_doc", title: "One GraphRAG document", read_only: true,
        use_: "one document with its chapter summaries and every chunk (text + summary); poll it after rag_add_document until `searchable` is true.",
        props: vec![project(), ("id", string("the document id (rag_docs lists them)"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_DOC}),
        returns: "{id, kind, title, path, state: string, searchable: boolean, error: string, chunks: integer, chapters: [{idx, title, summary}], summary, linked_nodes?: [node], chunk_list: [{id, idx, chapter, heading, text, summary, sum_state}]}",
        errors: errs("\"id is required\"; 404 \"no GraphRAG document <id>\"; "),
        limits: "searchable: keyword and raw-text search from summarizing on; summary search once ready. A failed document's error is generic with an error_id.",
        output: out(&[("id", t("string")), ("state", t("string")), ("searchable", t("boolean")), ("chunk_list", t("array"))]),
    }
}

fn rag_link_document() -> ToolDoc {
    ToolDoc {
        name: "rag_link_document", title: "Link a document to a map node", read_only: false,
        use_: "record that an uploaded document describes a map node (the node's children.documents gains it), so a search hit on either brings the other; unlink removes it.",
        props: vec![project(), ("doc", string("the GraphRAG document id (rag_docs lists them)")),
                    ("node", string("the node file ({topdir}/…/x.code.iter.md) or the node id (graph_lookup finds it)")),
                    ("unlink", boolean("remove the link instead", false))],
        required: &["doc", "node"],
        example: json!({"project": "pdy-dev", "doc": EX_DOC, "node": EX_NODE_PATH}),
        returns: "{node: string (id), document: string (path), linked: boolean, changed: boolean}",
        errors: errs("\"doc is required\" / \"node is required\"; 404 no such document or node; 400 \"only uploaded documents are linked to nodes\"; "),
        limits: "at once in the graph; the node file changes after an engine's next sync.",
        output: out(&[("node", t("string")), ("document", t("string")), ("linked", t("boolean")), ("changed", t("boolean"))]),
    }
}

fn rag_add_document() -> ToolDoc {
    ToolDoc {
        name: "rag_add_document", title: "Add a document to GraphRAG", read_only: false,
        use_: "index a document that is NOT a map node: `text` (UTF-8, e.g. markdown) or `content_b64` (pdf, docx, pptx, html, md, txt, csv, json, yaml…). Never for *.iter.md node files: engines index those from the checkout.",
        props: vec![
            project(),
            ("filename", string("e.g. design-notes.md: decides the format and the stored name")),
            ("text", string("the document as text; give this or content_b64; no default")),
            ("content_b64", string("the file's bytes, base64; no default")),
            ("title", string("no default (the filename)")),
            ("store", boolean("also commit the original to the project's docs directory", true)),
            ("wait_sec", integer("wait up to this many seconds for the document to become searchable", Some(0), 0, 60)),
        ],
        required: &["filename"],
        example: json!({"project": "pdy-dev", "filename": "clearing-retry-notes.md", "text": "# Clearing retries\nGateway 504s are retried twice.", "store": false, "wait_sec": 5}),
        returns: "{id: string, state: string (queued|ingesting|summarizing|rollup|ready|failed), searchable: boolean, live_engines: integer, waited_sec: integer, message: string (\"searchable now\" | \"still <state>: poll rag_doc …\" | \"no live engine serves this project: …\" | \"failed: …\")}",
        errors: errs("\"node files are indexed by the iter engine from the checkout; change the node with graph_node_update (or edit the file) instead\" — for *.iter.md; \"give `text` or `content_b64`\"; 400 too large (25 MB) or not a supported type; "),
        limits: "nothing is searchable until an engine has ingested it: one engine tick (5 s) plus extraction and embedding, ~5–30 s for a text under 1 MB; summaries follow minutes later. With live_engines 0 it stays queued until an engine serving the project checks in. A re-upload of the same filename replaces the document.",
        output: out(&[("id", t("string")), ("state", t("string")), ("searchable", t("boolean")), ("live_engines", t("integer")), ("waited_sec", t("integer")), ("message", t("string"))]),
    }
}

// ----- people only (PEOPLE_ONLY) -----

/// The note every people-only tool's Limits ends with.
const PEOPLE: &str = "a person's token only (engine sessions — iter's own agents — cannot list or call it); it acts as that person, and the item's history names them.";

fn people_errs(own: &str) -> String {
    errs(&format!("{own}403 when your role cannot write (a viewer); "))
}

fn whoami() -> ToolDoc {
    ToolDoc {
        name: "whoami", title: "Who am I", read_only: true,
        use_: "who this connection acts as on the project, and when its token expires — check it at the start of a session, and tell your person when token_days_left is under 14 (they mint a new token on their Settings page and replace it in the connector).",
        props: vec![project()],
        required: &[],
        example: json!({"project": "pdy-dev"}),
        returns: "{requestedby: string (user:<you>), sub: string, role: string (user|admin|viewer), token_expires: string (UTC, \"\" when unknown), token_days_left: integer|null, parent: null, by_workid: null}",
        errors: COMMON_ERRORS.into(),
        limits: "read at call time. ",
        output: out(&[("requestedby", t("string")), ("sub", t("string")), ("role", t("string")), ("token_expires", t("string")), ("token_days_left", opt("integer"))]),
    }
}

fn workitem_answer() -> ToolDoc {
    ToolDoc {
        name: "workitem_answer", title: "Answer a question item", read_only: false,
        use_: "answer a work item waiting on a person (state question, or one needing approval): fills its newest question form and queues the item again, exactly as the webui's Submit answer. Read the question first with workitem_details (the row whose value has `fields`).",
        props: vec![project(), ("id", string("the item: full id or a unique suffix")),
                    ("answer", string("one free-text answer, for a form with a single field (or an `answer` field); no default")),
                    ("answers", object("field key → value, for a form with several fields: text/combo a string, radio one option's value, checkbox a list of option values, int a whole number; no default"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "0a1b2c3d4e5f", "answer": "Retry on 502 and 504, twice each, then park with the gateway's response body."}),
        returns: "{id: string, state: \"queued\", question: string (the form's title), answers: {key: value} (every field as saved), message: string}",
        errors: people_errs("\"the item has no question to answer\"; \"this form has several fields: pass `answers` keyed by …\"; \"the form has no field X\"; \"the item is complete/failed: a closed item takes no answer (workitem_reopen it first)\"; \"the item is in progress\"; 400 when a value is not one of the field's options; 409 when the item changed meanwhile — read it again; "),
        limits: "the newest question form only; answering requeues the item and clears a needed approval. ",
        output: out(&[("id", t("string")), ("state", t("string")), ("answers", t("object")), ("message", t("string"))]),
    }
}

fn workitem_set_state() -> ToolDoc {
    ToolDoc {
        name: "workitem_set_state", title: "Change a work item's state", read_only: false,
        use_: "move one item to any state, as the webui's state menu: park or unpark (queued), pause or resume, close as complete or failed, or requeue; queueing also clears a needed approval, and run_now starts it even while the agent cap is full. To reopen a closed item use workitem_reopen; to stop a running one, workitem_stop.",
        props: vec![project(), ("id", string("the item: full id or a unique suffix")),
                    ("state", one_of("the new state", &["queued", "parked", "paused", "question", "scheduled", "complete", "failed", "in-progress"], None)),
                    ("reason", string("why, recorded on the item's history; no default")),
                    ("run_now", boolean("with state queued: start it on the engine's next tick even when the agent cap is full", false))],
        required: &["id", "state"],
        example: json!({"project": "pdy-dev", "id": "0e2f4a6c8b1d", "state": "parked", "reason": "waiting for the gateway vendor's answer"}),
        returns: "{id: string, was: string, state: string, run_now: boolean, message: string (what the change means: e.g. closing freezes the record, leaving in-progress does not stop the session)}",
        errors: people_errs("\"unknown state X\"; \"run_now goes with state queued\"; 409 when the item changed meanwhile — read it again; "),
        limits: "one item; closed states are complete and failed (frozen afterwards except notes, tags and reopen); in-progress is the engine's own state and starts no session. ",
        output: out(&[("id", t("string")), ("was", t("string")), ("state", t("string")), ("run_now", t("boolean")), ("message", t("string"))]),
    }
}

fn workitem_stop() -> ToolDoc {
    ToolDoc {
        name: "workitem_stop", title: "Stop a running item", read_only: false,
        use_: "stop a work item mid-run, as the webui's Stop: the engine ends the agent session on its next tick and parks the item with a note.",
        props: vec![project(), ("id", string("the running item: full id or a unique suffix"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "6f1c2a9b04de"}),
        returns: "{id: string, ok: boolean, message: string}",
        errors: people_errs("\"the item is X, not in-progress: nothing runs to stop\"; 409 when the item changed meanwhile — read it again; "),
        limits: "takes effect on the engine's next tick (seconds); work the agent already committed stays in git. ",
        output: out(&[("id", t("string")), ("ok", t("boolean")), ("message", t("string"))]),
    }
}

fn workitem_priority() -> ToolDoc {
    ToolDoc {
        name: "workitem_priority", title: "Re-prioritise a work item", read_only: false,
        use_: "set an open item's priority in place, as the webui's Set priority: lower runs sooner (0–9 do now, 10–39 use cases, 40–49 human, 50+ maintenance); the items it created follow it (children inherit exactly).",
        props: vec![project(), ("id", string("the item: full id or a unique suffix")), ("priority", integer("the new priority; no default", None, 0, 99))],
        required: &["id", "priority"],
        example: json!({"project": "pdy-dev", "id": "0e2f4a6c8b1d", "priority": 5}),
        returns: "{id: string, priority: integer, changed: [string] (every item of the lineage that moved)}",
        errors: people_errs("\"priority must be 0–99\"; 400 the item is closed; "),
        limits: "open items only; the whole lineage this item created moves with it. ",
        output: out(&[("id", t("string")), ("priority", t("integer")), ("changed", list(t("string")))]),
    }
}

fn workitem_reopen() -> ToolDoc {
    ToolDoc {
        name: "workitem_reopen", title: "Reopen a closed item", read_only: false,
        use_: "send a failed or complete item back to the queue (resolve a failure and retry it), as the webui's Reopen: queued again with its gate bounces and last error cleared, the reason recorded.",
        props: vec![project(), ("id", string("the closed item: full id or a unique suffix")), ("reason", string("why it is retried; recorded on its history; no default"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "1f2a3b4c5d6e", "reason": "the gateway credentials were rotated; retry"}),
        returns: "{id: string, state: \"queued\", message: string}",
        errors: people_errs("400 \"workitem is not closed\" — use workitem_set_state for open items; "),
        limits: "closed items only; items that wait on it are blocked again until it closes. ",
        output: out(&[("id", t("string")), ("state", t("string")), ("message", t("string"))]),
    }
}

fn workitem_edit() -> ToolDoc {
    ToolDoc {
        name: "workitem_edit", title: "Edit a work item", read_only: false,
        use_: "change an item's request text, name, agent, code paths, dependencies, tags, context or model, as the webui's edit form: only the arguments given change. State and priority have their own tools.",
        props: vec![project(), ("id", string("the item: full id or a unique suffix")),
                    ("name", string("the new one-line name; no default")), ("request", string("the new instructions (replaces the request text); no default")),
                    ("agent", string("the agent that runs it (code | plan | test | usecase | …); no default")),
                    ("codepaths", strings("the lock scope, replacing it: {topdir}/… or repo-relative paths; no default")),
                    ("depends_on", strings("ids or unique suffixes it waits for, replacing the list ([] clears it); no default")),
                    ("depends_on_shallow", boolean("wait only for the named items, not their children", false)),
                    ("tags", strings("the whole tag list, replacing it: text or text:#color; no default")),
                    ("add_tags", strings("tags to add: text or text:#color; no default")), ("remove_tags", strings("tag texts to remove; no default")),
                    ("context", strings("files or notes the agent reads first, replacing the list; no default")),
                    ("model", string("model override for its runs (\"\" = the agent's); no default"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "0e2f4a6c8b1d", "request": "Add a gateway client with a 10 s timeout and two retries. Done when its unit tests pass.", "add_tags": ["gateway"]}),
        returns: "{id: string, changed: [string] (the fields saved), message: string}",
        errors: people_errs("\"nothing to change\"; \"the item is in progress: workitem_stop it … before editing\"; \"an item cannot depend on itself\"; 400 dependency cycle; 409 the item is closed (tags only) or changed meanwhile — read it again; "),
        limits: "not while in progress; a closed item takes tag changes only. ",
        output: out(&[("id", t("string")), ("changed", list(t("string"))), ("message", t("string"))]),
    }
}

fn workitem_clone() -> ToolDoc {
    ToolDoc {
        name: "workitem_clone", title: "Clone, follow up or run a schedule now", read_only: false,
        use_: "file a new queued item from an existing one, as the webui's Clone, Follow-up and Run now: `copy` repeats it, `followup` files new work that waits for it (with the original request quoted), `run_now` starts one run of a schedule.",
        props: vec![project(), ("id", string("the source item: full id or a unique suffix")),
                    ("as", one_of("what to make", &["copy", "followup", "run_now"], Some("copy"))),
                    ("request", string("the new request text: required for followup (what to do now); for copy it replaces the original; no default")),
                    ("name", string("the new item's name; no default (the source's, or \"Follow-up: …\")"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "0e2f4a6c8b1d", "as": "followup", "request": "Use the new gateway client in the clearing export."}),
        returns: "{id: string (the new item), from: string, as: string, state: string, requestedby: string, already_open: boolean, warnings: [string]}",
        errors: people_errs("\"a follow-up needs `request`\"; \"run_now clones a scheduled item\"; \"`as` is copy, followup or run_now\"; "),
        limits: "the new item is queued with the source's agent, priority, code paths, tags and context; a repeat of an open item is returned as already_open. ",
        output: out(&[("id", t("string")), ("from", t("string")), ("as", t("string")), ("already_open", t("boolean"))]),
    }
}

fn workitem_duplicate_of() -> ToolDoc {
    ToolDoc {
        name: "workitem_duplicate_of", title: "Close an item as a duplicate", read_only: false,
        use_: "close one open item as a duplicate of another (the survivor): it closes complete, and the survivor records it as seen again, with its request as evidence.",
        props: vec![project(), ("id", string("the duplicate: full id or a unique suffix")), ("survivor", string("the item that stays: full id or a unique suffix")),
                    ("reason", string("one sentence: why they are the same; no default"))],
        required: &["id", "survivor"],
        example: json!({"project": "pdy-dev", "id": "2b3c4d5e6f70", "survivor": "0e2f4a6c8b1d", "reason": "both add the same gateway client"}),
        returns: "{id: string, state: \"complete\", survivor: string, survivor_repeats: integer, message: string}",
        errors: people_errs("409 \"the duplicate is …\" / \"the survivor is …\" (closed or in progress); 400 \"survivor must name another workitem\"; "),
        limits: "both open and not in progress; the survivor's priority is raised as for any repeat. ",
        output: out(&[("id", t("string")), ("survivor", t("string")), ("message", t("string"))]),
    }
}

fn workitem_explain() -> ToolDoc {
    ToolDoc {
        name: "workitem_explain", title: "Explain an item simply", read_only: false,
        use_: "ask for a plain-language explanation of an item (the webui's ELI5): a read-only agent writes it as an \"explained\" row on the item's history; read it with workitem_details.",
        props: vec![project(), ("id", string("the item: full id or a unique suffix"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "0e2f4a6c8b1d"}),
        returns: "{id: string, ok: boolean, message: string}",
        errors: people_errs(""),
        limits: "any state; needs a live engine serving the project; usually 30–60 s. ",
        output: out(&[("id", t("string")), ("ok", t("boolean")), ("message", t("string"))]),
    }
}

fn workitem_delete() -> ToolDoc {
    ToolDoc {
        name: "workitem_delete", title: "Delete a work item", read_only: false,
        use_: "remove an unwanted item for good, as the webui's Delete. To keep a record instead, close it with workitem_set_state (failed or complete) or workitem_duplicate_of.",
        props: vec![project(), ("id", string("the item: full id or a unique suffix"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": "3c4d5e6f7a81"}),
        returns: "{id: string, deleted: boolean}",
        errors: people_errs("409 \"the test sweep cannot be deleted: pause it to turn it off\"; "),
        limits: "cannot be undone; items that waited on it no longer do. ",
        output: out(&[("id", t("string")), ("deleted", t("boolean"))]),
    }
}

fn rag_delete_document() -> ToolDoc {
    ToolDoc {
        name: "rag_delete_document", title: "Delete a GraphRAG document", read_only: false,
        use_: "remove an uploaded document from the project's GraphRAG index (its chunks and links go too). To update a document instead, upload it again under the same filename with rag_add_document: that replaces it.",
        props: vec![project(), ("id", string("the document id (rag_docs lists them)")),
                    ("remove_file", boolean("also remove its stored original from the project's docs directory", false))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_DOC}),
        returns: "{id: string, removed: string, file_removal: object|null}",
        errors: people_errs("404 \"no GraphRAG document <id>\"; "),
        limits: "cannot be undone; node files (*.iter.md) come back on the engine's next sync — change the node instead. ",
        output: out(&[("id", t("string")), ("removed", t("string"))]),
    }
}

fn rag_resummarize_document() -> ToolDoc {
    ToolDoc {
        name: "rag_resummarize_document", title: "Redo a document's summaries", read_only: false,
        use_: "queue a document's chunk and document summaries to be written again (e.g. after the summary agent's prompt improved); its text and keyword search stay as they are meanwhile.",
        props: vec![project(), ("id", string("the document id (rag_docs lists them)"))],
        required: &["id"],
        example: json!({"project": "pdy-dev", "id": EX_DOC}),
        returns: "{id: string, result: {doc: string, chunks_reset: integer}, message: string}",
        errors: people_errs("404 \"no GraphRAG document <id>\"; "),
        limits: "an engine's Summary agent does the work (Haiku, billed to its account); rag_status shows the backlog. ",
        output: out(&[("id", t("string")), ("result", t("object")), ("message", t("string"))]),
    }
}

/// A tool's own refusals, then the families every project tool shares.
fn errs(own: &str) -> String {
    format!("{own}{COMMON_ERRORS}")
}
