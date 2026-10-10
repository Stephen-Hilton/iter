//! Frontmatter plumbing: split `---` fences, read YAML tolerantly into JSON
//! values, and write YAML deterministically (independent of serde_json's
//! `preserve_order` feature — every map is ordered explicitly here).

use super::Finding;
use serde_json::{Map, Value};

/// Split a node file into (frontmatter text, body). The body is everything
/// after the closing fence line, verbatim. `None` = no (terminated) fence.
pub(crate) fn split(text: &str) -> (Option<String>, String, bool) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = t.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, String::new(), false);
    };
    if first.trim_end() != "---" {
        return (None, t.to_string(), false);
    }
    let mut front = String::new();
    let mut consumed = first.len();
    for line in lines {
        consumed += line.len();
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            return (Some(front), t[consumed..].to_string(), false);
        }
        front.push_str(line.trim_end_matches(['\n', '\r']));
        front.push('\n');
    }
    // opening fence, never closed: not frontmatter
    (None, t.to_string(), true)
}

/// Read the frontmatter into a JSON map. Strict YAML first; when that fails,
/// each top-level key is read on its own (a `key: value` line whose value is
/// not valid YAML is taken as a plain string) and a `yaml-repaired` finding is
/// recorded.
pub(crate) fn load_map(front: &str, findings: &mut Vec<Finding>) -> Map<String, Value> {
    if front.trim().is_empty() {
        return Map::new();
    }
    match serde_yaml::from_str::<serde_yaml::Value>(front) {
        Ok(serde_yaml::Value::Mapping(m)) => {
            let mut out = Map::new();
            for (k, v) in m {
                out.insert(key_string(&k), to_json(v));
            }
            return out;
        }
        Ok(serde_yaml::Value::Null) => return Map::new(),
        Ok(_) => {
            findings.push(Finding::new("yaml-repaired", "frontmatter is not a key: value mapping"));
        }
        Err(e) => {
            // the common hand-edit slip: an unquoted `{placeholder}/path`
            // (YAML reads `{` as a flow mapping)
            let repaired = quote_placeholders(front);
            if repaired != front {
                if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(&repaired) {
                    findings.push(Finding::new("yaml-repaired", "quoted unquoted {placeholder} paths"));
                    let mut out = Map::new();
                    for (k, v) in m {
                        out.insert(key_string(&k), to_json(v));
                    }
                    return out;
                }
            }
            findings.push(Finding::new("yaml-repaired", format!("frontmatter is not valid YAML ({}); read key by key", e)));
        }
    }
    load_chunked(&quote_placeholders(front), findings)
}

const PLACEHOLDERS: &[&str] = &["{topdir}", "{thisfiledir}", "{thisfilestem}", "{thisfilename}", "{projectname}"];

/// Wrap every unquoted scalar that starts with a `{placeholder}` in double
/// quotes (up to the next `,` / `]` / `}` of a flow collection, or the end
/// of the line).
pub(crate) fn quote_placeholders(front: &str) -> String {
    let mut out = String::with_capacity(front.len() + 16);
    for line in front.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        out.push_str(&quote_line(body));
        out.push_str(nl);
    }
    out
}

fn quote_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    let mut i = 0;
    let mut in_q: Option<char> = None;
    let mut depth = 0i32; // flow [ ] nesting
    let mut prev_sig: Option<char> = None; // previous non-space char outside quotes
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_q {
            out.push(c);
            if c == '\\' && q == '"' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                in_q = None;
                prev_sig = Some(c);
            }
            i += 1;
            continue;
        }
        let token_start = matches!(prev_sig, None | Some('[') | Some(',') | Some(':') | Some('-'));
        if c == '{' && token_start {
            let rest: String = chars[i..].iter().collect();
            if PLACEHOLDERS.iter().any(|p| rest.starts_with(p)) {
                // token end
                let mut j = i;
                while j < chars.len() {
                    let d = chars[j];
                    if depth > 0 && (d == ',' || d == ']') {
                        break;
                    }
                    if depth == 0 && d == ' ' && j + 1 < chars.len() && chars[j + 1] == '#' {
                        break;
                    }
                    j += 1;
                }
                let tok: String = chars[i..j].iter().collect::<String>();
                let tok = tok.trim_end();
                out.push_str(&quote(tok));
                // keep the whitespace we trimmed
                let consumed = tok.chars().count();
                i += consumed;
                prev_sig = Some('"');
                continue;
            }
        }
        match c {
            '"' | '\'' if token_start => in_q = Some(c),
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        out.push(c);
        if !c.is_whitespace() {
            prev_sig = Some(c);
        }
        i += 1;
    }
    out
}

fn load_chunked(front: &str, findings: &mut Vec<Finding>) -> Map<String, Value> {
    // chunk = a column-0 line plus its indented / list continuation lines
    let mut chunks: Vec<Vec<&str>> = Vec::new();
    for line in front.lines() {
        let starts_chunk = !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.starts_with('-')
            && !line.starts_with('#');
        if starts_chunk || chunks.is_empty() {
            chunks.push(vec![line]);
        } else {
            chunks.last_mut().unwrap().push(line);
        }
    }
    let mut out = Map::new();
    for chunk in chunks {
        let text = chunk.join("\n");
        if text.trim().is_empty() || (text.trim_start().starts_with('#') && chunk.len() == 1) {
            continue;
        }
        if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(&text) {
            for (k, v) in m {
                out.insert(key_string(&k), to_json(v));
            }
            continue;
        }
        let first = chunk[0];
        match first.split_once(':') {
            Some((k, rest)) if !k.trim().is_empty() && !k.trim().contains(' ') => {
                let k = k.trim().trim_matches(|c| c == '"' || c == '\'');
                let v = strip_quotes(rest.trim());
                if chunk.len() > 1 {
                    findings.push(Finding::new(
                        "yaml-repaired",
                        format!("key {:?}: kept the first line only; {} continuation line(s) dropped", k, chunk.len() - 1),
                    ));
                }
                out.insert(k.to_string(), Value::String(v.to_string()));
            }
            _ => findings.push(Finding::new("yaml-repaired", format!("dropped unreadable frontmatter line {:?}", first))),
        }
    }
    out
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn key_string(k: &serde_yaml::Value) -> String {
    match k {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Null => String::new(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

pub(crate) fn to_json(v: serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
            }
        }
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Sequence(seq) => Value::Array(seq.into_iter().map(to_json).collect()),
        serde_yaml::Value::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                out.insert(key_string(&k), to_json(v));
            }
            Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => to_json(t.value),
    }
}

/* ------------------------------------------------------------- emitting */

/// Keys of nested maps in a readable, fixed order (flowmap, flow steps,
/// connects, test results); anything else follows alphabetically.
const PREFERRED: &[&str] = &[
    "id", "name", "summary", "sequence", "process_flow", "data_flow", "step", "from", "to", "what", "data",
    "stored", "plain", "evidence", "overall_success", "normal", "longtail", "failure", "total", "pass", "err",
    "details", "bucket", "msg",
];

pub(crate) fn ordered_keys(m: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort_by(|a, b| {
        let ra = PREFERRED.iter().position(|p| p == a).unwrap_or(usize::MAX);
        let rb = PREFERRED.iter().position(|p| p == b).unwrap_or(usize::MAX);
        ra.cmp(&rb).then_with(|| a.cmp(b))
    });
    keys
}

const RESERVED: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "y", "n"];

/// A string YAML reads back as the same string without quotes.
fn plain_safe(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !RESERVED.contains(&s)
}

/// Double-quoted YAML scalar (escapes everything YAML does not print).
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20
                || (0x7f..=0x9f).contains(&(c as u32))
                || c == '\u{feff}'
                || c == '\u{fffe}'
                || c == '\u{ffff}' =>
            {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A string: plain when unambiguous, else double-quoted.
pub(crate) fn scalar_str(s: &str) -> String {
    if plain_safe(s) { s.to_string() } else { quote(s) }
}

pub(crate) fn yaml_key(k: &str) -> String {
    let ok = !k.is_empty()
        && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !RESERVED.contains(&k.to_ascii_lowercase().as_str());
    if ok { k.to_string() } else { quote(k) }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => scalar_str(s),
        _ => unreachable!(),
    }
}

/// `[a, "b c"]` — a flow list of scalars.
pub(crate) fn flow_list<'a>(items: impl IntoIterator<Item = &'a Value>) -> String {
    let parts: Vec<String> = items.into_iter().map(scalar).collect();
    format!("[{}]", parts.join(", "))
}

/// `["a", "b"]` — a flow list of strings, always quoted (children lists).
pub(crate) fn flow_strings(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| quote(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// One `key: value` entry at `indent` spaces, nested values below it.
pub(crate) fn emit_entry(out: &mut String, indent: usize, key: &str, v: &Value) {
    let pad = " ".repeat(indent);
    let k = yaml_key(key);
    match v {
        Value::Array(a) if a.iter().all(is_scalar) => {
            out.push_str(&format!("{}{}: {}\n", pad, k, flow_list(a)));
        }
        Value::Array(a) => {
            out.push_str(&format!("{}{}:\n", pad, k));
            emit_seq(out, indent + 2, a);
        }
        Value::Object(m) if m.is_empty() => out.push_str(&format!("{}{}: {{}}\n", pad, k)),
        Value::Object(m) => {
            out.push_str(&format!("{}{}:\n", pad, k));
            emit_map(out, indent + 2, m);
        }
        s => out.push_str(&format!("{}{}: {}\n", pad, k, scalar(s))),
    }
}

pub(crate) fn emit_map(out: &mut String, indent: usize, m: &Map<String, Value>) {
    for k in ordered_keys(m) {
        emit_entry(out, indent, k, &m[k]);
    }
}

fn emit_seq(out: &mut String, indent: usize, a: &[Value]) {
    let pad = " ".repeat(indent);
    for item in a {
        match item {
            Value::Array(inner) if inner.iter().all(is_scalar) => {
                out.push_str(&format!("{}- {}\n", pad, flow_list(inner)));
            }
            Value::Array(inner) => {
                out.push_str(&format!("{}-\n", pad));
                emit_seq(out, indent + 2, inner);
            }
            Value::Object(m) if m.is_empty() => out.push_str(&format!("{}- {{}}\n", pad)),
            Value::Object(m) => {
                let mut sub = String::new();
                emit_map(&mut sub, indent + 2, m);
                // the first entry goes on the dash line
                let inner_pad = " ".repeat(indent + 2);
                let rest = sub.strip_prefix(&inner_pad).unwrap_or(&sub);
                out.push_str(&format!("{}- {}", pad, rest));
            }
            s => out.push_str(&format!("{}- {}\n", pad, scalar(s))),
        }
    }
}
