//! Work-item queries (MCP/GraphRAG hardening, MG-8 to MG-10): filter on any
//! field of the stored record, sort on any field, page with a cursor. Pure
//! functions, no I/O; `POST /api/projects/{p}/workitems/query` (api.rs) runs
//! them over the project's items.
//!
//! - **Paths** are dotted (`ts.receive`, `tags.text`). A path that crosses a
//!   list yields every element; a positive clause matches when any element
//!   does, `ne` / `nin` only when none does.
//! - **A missing field, or one holding null (or an empty list), reads as
//!   FALSE/empty**: it equals `false`, `""`, `[]` and `null`, is `empty` and
//!   not `exists`, matches `ne` against any non-empty value, never matches
//!   `gt` / `gte` / `lt` / `lte` / `contains` / `prefix`, and matches `in`
//!   only when the list holds `false`, `""` or `null`.
//! - **Sort**: missing sorts lowest (last in a descending sort); types order
//!   null < boolean < number < string < list < object; strings by byte (ISO
//!   times sort right); a list sorts by its first element; `id` ascending is
//!   always the last key, so the order is total.
//! - **Cursor**: the last item's sort key, bound to the query (a hash of the
//!   filter and sort), so an item created between pages neither shifts a
//!   page nor repeats, and a cursor sent with another query is refused.

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

pub const OPS: &[&str] = &["eq", "ne", "in", "nin", "gt", "gte", "lt", "lte", "contains", "prefix", "exists", "empty"];
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;
pub const CURSOR_ELSEWHERE: &str = "cursor belongs to another query";

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Clause { field: String, op: String, value: Value },
    Any(Vec<Filter>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub field: String,
    pub desc: bool,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub filter: Vec<Filter>,
    pub sort: Vec<SortKey>,
    pub limit: usize,
    pub cursor: Option<String>,
    pub fields: Vec<String>,
}

fn clause(v: &Value) -> Result<Filter, String> {
    if let Some(any) = v.get("any") {
        let list = any.as_array().ok_or("`any` must be a list of clauses")?;
        return Ok(Filter::Any(list.iter().map(clause).collect::<Result<_, _>>()?));
    }
    let field = v.get("field").and_then(|f| f.as_str()).map(str::trim).unwrap_or("");
    if field.is_empty() {
        return Err(format!("a filter clause needs a field: {v}"));
    }
    let op = v.get("op").and_then(|o| o.as_str()).unwrap_or("eq");
    if !OPS.contains(&op) {
        return Err(format!("unknown op {op} (one of {})", OPS.join(", ")));
    }
    let value = match (op, v.get("value")) {
        ("exists" | "empty", None) => json!(true),
        ("exists" | "empty", Some(b)) if !b.is_boolean() => return Err(format!("{op} takes true or false")),
        ("in" | "nin", Some(l)) if !l.is_array() => return Err(format!("{op} takes a list")),
        (_, None) => return Err(format!("the clause on {field} needs a value")),
        (_, Some(x)) => x.clone(),
    };
    Ok(Filter::Clause { field: field.to_string(), op: op.to_string(), value })
}

/// The request body: `filter`, `sort`, `limit`, `cursor`, `fields`, and the
/// shortcuts `state`, `tag` and `text` (a substring of the name).
pub fn parse(body: &Value) -> Result<Query, String> {
    let mut filter = match body.get("filter") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.iter().map(clause).collect::<Result<_, _>>()?,
        Some(one @ Value::Object(_)) => vec![clause(one)?],
        Some(_) => return Err("filter must be a list of {field, op, value} clauses".into()),
    };
    let short = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    if let Some(s) = short("state") {
        filter.push(Filter::Clause { field: "state".into(), op: "eq".into(), value: json!(s) });
    }
    if let Some(t) = short("tag") {
        filter.push(Filter::Clause { field: "tags.text".into(), op: "eq".into(), value: json!(t) });
    }
    if let Some(t) = short("text") {
        filter.push(Filter::Clause { field: "name".into(), op: "contains".into(), value: json!(t) });
    }
    let mut sort = Vec::new();
    for k in body.get("sort").and_then(|s| s.as_array()).into_iter().flatten() {
        let field = k.get("field").and_then(|f| f.as_str()).map(str::trim).unwrap_or("");
        if field.is_empty() {
            return Err(format!("a sort key needs a field: {k}"));
        }
        let desc = match k.get("dir").and_then(|d| d.as_str()).unwrap_or("asc") {
            "asc" => false,
            "desc" => true,
            other => return Err(format!("sort dir is asc or desc (got {other})")),
        };
        sort.push(SortKey { field: field.to_string(), desc });
    }
    if sort.is_empty() {
        sort.push(SortKey { field: "ts.receive".into(), desc: true });
    }
    if !sort.iter().any(|k| k.field == "id") {
        sort.push(SortKey { field: "id".into(), desc: false });
    }
    let limit = match body.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(l) => l.as_u64().ok_or("limit is an integer 1–500")?.clamp(1, MAX_LIMIT as u64) as usize,
    };
    let cursor = body.get("cursor").and_then(|c| c.as_str()).map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    let fields = body.get("fields").and_then(|f| f.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    Ok(Query { filter, sort, limit, cursor, fields })
}

/// Every value at a dotted path, lists crossed on the way expanded; a final
/// list is one value. Missing keys give nothing; nulls are kept (callers
/// that read "missing" drop them).
pub fn get_path<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut cur: Vec<&Value> = vec![v];
    for seg in path.split('.') {
        let mut next = Vec::new();
        for c in cur {
            match c {
                Value::Object(o) => next.extend(o.get(seg)),
                Value::Array(a) => next.extend(a.iter().filter_map(|e| e.get(seg))),
                _ => {}
            }
        }
        cur = next;
    }
    cur
}

/// The present values at a path: nulls and empty lists read as missing.
fn present<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    get_path(v, path).into_iter().filter(|x| !x.is_null() && x.as_array().map(|a| !a.is_empty()).unwrap_or(true)).collect()
}

/// false, "", [], {} and null: what a missing field equals.
fn blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Number(_) => false,
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

/// A present value equals `x`: itself, or (a list) one of its elements.
fn equals(v: &Value, x: &Value) -> bool {
    same(v, x) || v.as_array().map(|a| a.iter().any(|e| same(e, x))).unwrap_or(false)
}

/// A value and, for a list, its elements: what ordering and string ops see.
fn leaves(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        x => vec![x],
    }
}

fn rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

/// The MG-10 total order of two JSON values.
pub fn cmp_values(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => x.as_f64().unwrap_or(0.0).partial_cmp(&y.as_f64().unwrap_or(0.0)).unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                let o = cmp_values(p, q);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Value::Object(x), Value::Object(y)) => serde_json::to_string(x).unwrap_or_default().cmp(&serde_json::to_string(y).unwrap_or_default()),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Whether a record matches one filter entry (the MG-9 rules).
pub fn matches(rec: &Value, f: &Filter) -> bool {
    let (field, op, x) = match f {
        Filter::Any(list) => return list.iter().any(|g| matches(rec, g)),
        Filter::Clause { field, op, value } => (field, op.as_str(), value),
    };
    let vals = present(rec, field);
    let missing = vals.is_empty();
    let list = || x.as_array().cloned().unwrap_or_default();
    let ordered = |want: &[Ordering]| vals.iter().flat_map(|v| leaves(v)).any(|v| rank(v) == rank(x) && want.contains(&cmp_values(v, x)));
    match op {
        "eq" => if missing { blank(x) } else { vals.iter().any(|v| equals(v, x)) },
        "ne" => if missing { !blank(x) } else { !vals.iter().any(|v| equals(v, x)) },
        "in" => if missing { list().iter().any(|e| e.is_null() || e == &json!(false) || e == &json!("")) } else { list().iter().any(|e| vals.iter().any(|v| equals(v, e))) },
        "nin" => !matches(rec, &Filter::Clause { field: field.clone(), op: "in".into(), value: x.clone() }),
        "gt" => ordered(&[Ordering::Greater]),
        "gte" => ordered(&[Ordering::Greater, Ordering::Equal]),
        "lt" => ordered(&[Ordering::Less]),
        "lte" => ordered(&[Ordering::Less, Ordering::Equal]),
        "contains" => vals.iter().any(|v| match (v, x) {
            (Value::String(s), Value::String(n)) => s.to_lowercase().contains(&n.to_lowercase()),
            (Value::Array(a), _) => a.iter().any(|e| same(e, x)),
            _ => false,
        }),
        "prefix" => {
            let Some(p) = x.as_str() else { return false };
            vals.iter().flat_map(|v| leaves(v)).any(|v| v.as_str().map(|s| s.starts_with(p)).unwrap_or(false))
        }
        "exists" => missing != x.as_bool().unwrap_or(true),
        "empty" => (missing || vals.iter().all(|v| blank(v))) == x.as_bool().unwrap_or(true),
        _ => false,
    }
}

/// A record's value for one sort key: the first present value, a list by its
/// first element, missing as null.
fn sort_value(rec: &Value, field: &str) -> Value {
    let mut v = present(rec, field).into_iter().next().cloned().unwrap_or(Value::Null);
    while let Value::Array(a) = &v {
        v = a.first().cloned().unwrap_or(Value::Null);
    }
    v
}

pub fn sort_key(rec: &Value, sort: &[SortKey]) -> Vec<Value> {
    sort.iter().map(|k| sort_value(rec, &k.field)).collect()
}

fn cmp_keys(a: &[Value], b: &[Value], sort: &[SortKey]) -> Ordering {
    for (i, k) in sort.iter().enumerate() {
        let o = cmp_values(a.get(i).unwrap_or(&Value::Null), b.get(i).unwrap_or(&Value::Null));
        let o = if k.desc { o.reverse() } else { o };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Two records in the query's order (`id` is always the last key).
pub fn cmp(a: &Value, b: &Value, sort: &[SortKey]) -> Ordering {
    cmp_keys(&sort_key(a, sort), &sort_key(b, sort), sort)
}

fn filter_json(f: &Filter) -> Value {
    match f {
        Filter::Clause { field, op, value } => json!({"field": field, "op": op, "value": value}),
        Filter::Any(l) => json!({"any": l.iter().map(filter_json).collect::<Vec<_>>()}),
    }
}

/// The query's identity for its cursors: a hash of its filter and sort.
fn query_hash(q: &Query) -> String {
    let canon = json!({"filter": q.filter.iter().map(filter_json).collect::<Vec<_>>(), "sort": sort_json(&q.sort)});
    Sha256::digest(canon.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sort_json(sort: &[SortKey]) -> Value {
    json!(sort.iter().map(|k| json!({"field": k.field, "dir": if k.desc { "desc" } else { "asc" }})).collect::<Vec<_>>())
}

pub fn encode_cursor(q: &Query, last: &Value) -> String {
    let c = json!({"v": 1, "q": query_hash(q), "k": sort_key(last, &q.sort), "id": last.get("id").cloned().unwrap_or(Value::Null)});
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(c.to_string())
}

/// The sort key a cursor resumes after; refused when it is another query's.
pub fn decode_cursor(q: &Query, cursor: &str) -> Result<Vec<Value>, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor.trim()).map_err(|_| "cursor is not one this server issued".to_string())?;
    let c: Value = serde_json::from_slice(&bytes).map_err(|_| "cursor is not one this server issued".to_string())?;
    if c["v"] != 1 || c["k"].as_array().map(|k| k.len()) != Some(q.sort.len()) {
        return Err("cursor is not one this server issued".into());
    }
    if c["q"].as_str() != Some(query_hash(q).as_str()) {
        return Err(CURSOR_ELSEWHERE.into());
    }
    Ok(c["k"].as_array().cloned().unwrap_or_default())
}

/// The default shape of a listed item, plus any requested `fields`
/// (`["*"]`: the whole record, with `received`).
pub fn project_item(rec: &Value, fields: &[String]) -> Value {
    let received = rec.pointer("/ts/receive").cloned().unwrap_or(Value::Null);
    if fields.iter().any(|f| f == "*") {
        let mut whole = rec.clone();
        whole["received"] = received;
        return whole;
    }
    let mut out = json!({"id": rec["id"], "name": rec["name"], "state": rec["state"], "agent": rec["agent"], "priority": rec["priority"],
        "tags": rec["tags"].as_array().map(|t| t.iter().map(|x| x["text"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "blockedby": rec.get("blockedby").cloned().unwrap_or(json!([])), "received": received});
    for f in fields {
        out[f.as_str()] = pick(rec, &f.split('.').collect::<Vec<_>>());
    }
    out
}

/// The value at a path, keeping the shape of any list it crosses.
fn pick(v: &Value, segs: &[&str]) -> Value {
    match (segs.first(), v) {
        (None, x) => x.clone(),
        (Some(s), Value::Object(o)) => o.get(*s).map(|x| pick(x, &segs[1..])).unwrap_or(Value::Null),
        (Some(_), Value::Array(a)) => Value::Array(a.iter().map(|e| pick(e, segs)).collect()),
        _ => Value::Null,
    }
}

/// Filter and sort paths no record has (a typo shows instead of silently
/// matching everything).
pub fn unknown_fields(rows: &[Value], q: &Query) -> Vec<String> {
    fn paths(f: &Filter, out: &mut Vec<String>) {
        match f {
            Filter::Clause { field, .. } => out.push(field.clone()),
            Filter::Any(l) => l.iter().for_each(|g| paths(g, out)),
        }
    }
    let mut all = Vec::new();
    q.filter.iter().for_each(|f| paths(f, &mut all));
    all.extend(q.sort.iter().map(|k| k.field.clone()));
    let mut out: Vec<String> = Vec::new();
    for p in all {
        if !out.contains(&p) && !rows.iter().any(|r| !get_path(r, &p).is_empty()) {
            out.push(p);
        }
    }
    out
}

/// (total matching, the page, next cursor, unknown fields).
pub type Page = (usize, Vec<Value>, Option<String>, Vec<String>);

/// One page of `rows`.
pub fn run(rows: Vec<Value>, q: &Query) -> Result<Page, String> {
    let after = q.cursor.as_deref().map(|c| decode_cursor(q, c)).transpose()?;
    let unknown = unknown_fields(&rows, q);
    let mut hits: Vec<Value> = rows.into_iter().filter(|r| q.filter.iter().all(|f| matches(r, f))).collect();
    let total = hits.len();
    hits.sort_by(|a, b| cmp(a, b, &q.sort));
    let start = match &after {
        Some(k) => hits.iter().position(|r| cmp_keys(&sort_key(r, &q.sort), k, &q.sort) == Ordering::Greater).unwrap_or(hits.len()),
        None => 0,
    };
    let page: Vec<Value> = hits[start..].iter().take(q.limit).cloned().collect();
    let next = if start + page.len() < hits.len() { page.last().map(|l| encode_cursor(q, l)) } else { None };
    Ok((total, page.iter().map(|r| project_item(r, &q.fields)).collect(), next, unknown))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(field: &str, op: &str, value: Value) -> Filter {
        Filter::Clause { field: field.into(), op: op.into(), value }
    }

    #[test]
    fn every_operator_and_the_missing_rule() {
        let rec = json!({"id": "a", "state": "queued", "priority": 40, "name": "Fix the Login page",
                         "ts": {"receive": "2026-10-05T10:00:00Z"}, "tags": [{"text": "ui"}, {"text": "usecase:login"}],
                         "blockedby": ["b1", "b2"], "done": false, "note": null, "empty_list": []});
        let table: Vec<(Filter, bool)> = vec![
            (c("ts.receive", "gte", json!("2026-10-01")), true),
            (c("ts.receive", "lt", json!("2026-10-01")), false),
            (c("tags.text", "eq", json!("ui")), true),
            (c("tags.text", "eq", json!("api")), false),
            (c("tags.text", "ne", json!("ui")), false),
            (c("tags.text", "ne", json!("api")), true),
            (c("tags.text", "prefix", json!("usecase:")), true),
            (c("blockedby", "contains", json!("b2")), true),
            (c("blockedby", "contains", json!("b")), false),
            (c("blockedby", "eq", json!("b1")), true),
            (c("name", "contains", json!("login")), true),
            (c("name", "prefix", json!("Fix")), true),
            (c("priority", "in", json!([30, 40])), true),
            (c("priority", "nin", json!([30, 40])), false),
            (c("priority", "gt", json!(39)), true),
            (c("priority", "gt", json!("39")), false),
            (c("priority", "lte", json!(40)), true),
            (c("done", "eq", json!(false)), true),
            // missing, null and [] read as FALSE/empty
            (c("parked_reason", "eq", json!(false)), true),
            (c("parked_reason", "eq", json!("")), true),
            (c("parked_reason", "eq", json!([])), true),
            (c("parked_reason", "eq", json!(null)), true),
            (c("parked_reason", "eq", json!("x")), false),
            (c("parked_reason", "empty", json!(true)), true),
            (c("parked_reason", "exists", json!(true)), false),
            (c("parked_reason", "exists", json!(false)), true),
            (c("parked_reason", "gt", json!("")), false),
            (c("parked_reason", "contains", json!("")), false),
            (c("parked_reason", "prefix", json!("")), false),
            (c("parked_reason", "ne", json!("x")), true),
            (c("parked_reason", "ne", json!(false)), false),
            (c("parked_reason", "in", json!([false])), true),
            (c("parked_reason", "in", json!(["x"])), false),
            (c("parked_reason", "nin", json!(["x"])), true),
            (c("note", "exists", json!(true)), false),
            (c("empty_list", "eq", json!([])), true),
            (c("empty_list", "empty", json!(true)), true),
            (c("blockedby", "empty", json!(true)), false),
            (c("state", "empty", json!(false)), true),
            (Filter::Any(vec![c("state", "eq", json!("complete")), c("priority", "eq", json!(40))]), true),
            (Filter::Any(vec![c("state", "eq", json!("complete")), c("priority", "eq", json!(41))]), false),
        ];
        for (f, want) in table {
            assert_eq!(matches(&rec, &f), want, "{f:?}");
        }
    }

    #[test]
    fn parse_shortcuts_defaults_and_refusals() {
        let q = parse(&json!({"state": "queued", "tag": "ui", "text": "login"})).unwrap();
        assert_eq!(q.filter, vec![c("state", "eq", json!("queued")), c("tags.text", "eq", json!("ui")), c("name", "contains", json!("login"))]);
        assert_eq!(q.sort, vec![SortKey { field: "ts.receive".into(), desc: true }, SortKey { field: "id".into(), desc: false }]);
        assert_eq!((q.limit, parse(&json!({"limit": 9000})).unwrap().limit), (50, 500));
        assert!(parse(&json!({"filter": [{"field": "x", "op": "like", "value": 1}]})).unwrap_err().contains("unknown op like"));
        assert!(parse(&json!({"filter": [{"op": "eq", "value": 1}]})).is_err());
        assert!(parse(&json!({"filter": [{"field": "x", "op": "in", "value": 1}]})).is_err());
        assert!(parse(&json!({"sort": [{"field": "x", "dir": "up"}]})).is_err());
    }

    #[test]
    fn sort_order_types_missing_and_lists() {
        let q = parse(&json!({"sort": [{"field": "v", "dir": "asc"}]})).unwrap();
        let rows: Vec<Value> = [json!({"id": "s", "v": "b"}), json!({"id": "n", "v": 2}), json!({"id": "m"}), json!({"id": "t", "v": true}),
                                json!({"id": "l", "v": [1, 9]}), json!({"id": "o", "v": {"a": 1}}), json!({"id": "n0", "v": 1.5})]
            .into();
        let mut r = rows.clone();
        r.sort_by(|a, b| cmp(a, b, &q.sort));
        let ids: Vec<&str> = r.iter().map(|x| x["id"].as_str().unwrap()).collect();
        // missing < bool < numbers (a list by its first element) < string < object
        assert_eq!(ids, ["m", "t", "l", "n0", "n", "s", "o"]);
        let d = parse(&json!({"sort": [{"field": "v", "dir": "desc"}]})).unwrap();
        r.sort_by(|a, b| cmp(a, b, &d.sort));
        assert_eq!(r.last().unwrap()["id"], "m", "missing comes last in a descending sort");
    }

    #[test]
    fn pages_cover_everything_and_cursors_are_bound() {
        let rows: Vec<Value> = (0..25).map(|i| json!({"id": format!("id{i:02}"), "ts": {"receive": format!("2026-10-{:02}", 1 + i % 3)}})).collect();
        let q = |c: Option<String>| Query { cursor: c, limit: 10, ..parse(&json!({})).unwrap() };
        let mut seen = Vec::new();
        let mut cur = None;
        loop {
            let (total, page, next, _) = run(rows.clone(), &q(cur.clone())).unwrap();
            assert_eq!(total, 25);
            seen.extend(page.iter().map(|p| p["id"].as_str().unwrap().to_string()));
            match next {
                Some(n) => cur = Some(n),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!((seen.len(), sorted.len()), (25, 25));
        let (_, _, next, _) = run(rows.clone(), &q(None)).unwrap();
        let other = Query { cursor: next, ..parse(&json!({"state": "queued", "limit": 10})).unwrap() };
        assert_eq!(run(rows, &other).unwrap_err(), CURSOR_ELSEWHERE);
    }

    #[test]
    fn unknown_fields_and_projection() {
        let rows = vec![json!({"id": "a", "name": "n", "ts": {"receive": "t", "start": "s"}, "tags": [{"text": "x"}], "requestedby": "user:al"})];
        let q = parse(&json!({"filter": [{"field": "stat", "op": "eq", "value": "queued"}], "fields": ["ts.start", "requestedby", "tags.text"]})).unwrap();
        assert_eq!(unknown_fields(&rows, &q), vec!["stat".to_string()]);
        let item = project_item(&rows[0], &q.fields);
        assert_eq!((item["received"].as_str(), item["ts.start"].as_str(), item["requestedby"].as_str()), (Some("t"), Some("s"), Some("user:al")));
        assert_eq!(item["tags.text"], json!(["x"]));
        assert!(item.get("created").is_none());
        assert_eq!(project_item(&rows[0], &["*".into()])["ts"]["start"], "s");
    }
}
