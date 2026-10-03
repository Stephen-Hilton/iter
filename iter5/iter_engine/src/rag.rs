//! GraphRAG, the engine half (iter4, 2026-09-29; server half in
//! iter_data/src/rag/). The engine owns the heavy work — it has the checkout,
//! the Claude accounts, and native hardware (decided 2026-09-29):
//!
//! 1. `iter rag sync` — the change sweep. Reads the stored map's vertices,
//!    reads each node file (`*.iter.md`) from this checkout, and for those
//!    whose sha256 differs from what is indexed: chunks the text in the
//!    embedding model's own tokens and embeds every chunk (`prepare`), then
//!    sends chunks + vectors, plus the full list of node paths so documents
//!    of deleted or renamed files are dropped. Run after every map push that
//!    changed something (`Engine::sync_maps`), by the scheduled "GraphRAG
//!    change sweep" exec item, or by hand.
//!
//! 2. The **GraphRAG worker** — when the heartbeat reply's `rag_waiting` says
//!    a served project has work, workers claim jobs on their own threads,
//!    outside the agent cap and the queue:
//!    - **ingest**: an uploaded file — extract its text (`iter_rag::extract`;
//!      a scanned PDF is rendered with `pdftoppm` and read page by page by a
//!      Claude session, `ocr_pdf`), prepare it, PUT the chunks;
//!    - **chunks** / **rollup**: the **Summary agent**, one short tool-less
//!      `claude -p` session (Haiku unless the `summary` agent record says
//!      otherwise); each summary is embedded here before it is reported.
//!    A holding engine (no account under its stop%) runs none of it.

use crate::client::Api;
use crate::sync::Conn;
use iter_core::Project;
use serde_json::{Value, json};
use iter_rag::{chunk, embed, extract};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Node files per PUT (each carries its chunks and their vectors).
const SYNC_BATCH: usize = 25;
/// Pages a scanned PDF is OCR'd up to, and pages per Claude session.
pub const OCR_MAX_PAGES: usize = 120;
const OCR_PAGES_PER_CALL: usize = 6;
/// Jobs one worker thread runs before handing back to the tick (which
/// re-checks the account and starts another if work remains).
const JOBS_PER_RUN: usize = 25;

pub const DEFAULT_MODEL: &str = "haiku";
/// Chunks per Summary agent call unless the `summary` agent record sets `batch`.
pub const DEFAULT_BATCH: u64 = 16;
/// One rollup prompt's text at most: bigger documents are rolled up in
/// chapter windows first, then from their chapter summaries.
pub const ROLLUP_WINDOW_CHARS: usize = 40_000;
pub const DEFAULT_TIMEOUT_SEC: u64 = 300;

/// Bumped whenever chunking or embedding input changes: part of every node
/// file's hash, so the next sync re-indexes everything (summaries of chunks
/// whose text is unchanged are kept). v2 (2026-09-29): token-sized chunks.
pub const INDEX_VERSION: &str = "v2";

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

// ---------- 1. the change sweep ----------

/// `iter rag sync [--dry-run] [--force]`: exit 0 done, 1 some files failed, 2 could not run.
pub fn sync_verb(c: &Conn, dry_run: bool, force: bool) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter rag sync: no iter_data connection (ITER_DATA_URL / .iter/config.json + a token)");
        return 2;
    };
    match sync(api, &c.project, &c.topdir, dry_run, force) {
        Ok(r) => {
            println!(
                "GraphRAG sync {}: {} node file(s) in the map, {} changed/new sent{}, {} unreadable — indexed: +{} ~{} ={} -{}{}",
                c.project, r.total, r.sent, if dry_run { " (dry run: nothing sent)" } else { "" }, r.unreadable.len(),
                r.added, r.changed, r.unchanged, r.removed,
                if r.failed.is_empty() { String::new() } else { format!(", {} failed", r.failed.len()) }
            );
            for u in &r.unreadable {
                println!("  unreadable: {u}");
            }
            for f in &r.failed {
                println!("  failed: {} — {}", f["path"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or(""));
            }
            if r.failed.is_empty() { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("iter rag sync: {e}");
            2
        }
    }
}

#[derive(Default, Debug)]
pub struct SyncReport {
    pub total: usize,
    pub sent: usize,
    pub unreadable: Vec<String>,
    pub added: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub removed: u64,
    pub failed: Vec<Value>,
}

/// `{topdir}/a/b.code.iter.md` → the file under `topdir`.
fn file_of(topdir: &Path, path: &str) -> std::path::PathBuf {
    topdir.join(path.trim_start_matches("{topdir}").trim_start_matches('/'))
}

/// Node text as indexed: a heading naming the node, its description, then
/// the body with the frontmatter removed (the frontmatter is structure the
/// map already holds; the description is the part worth retrieving).
pub fn node_text(name: &str, nodetype: &str, description: &str, raw: &str) -> String {
    let (_, body) = chunk::split_frontmatter(raw);
    let head = format!("# {} ({nodetype})\n\n{}\n", if name.is_empty() { "(unnamed)" } else { name }, description.trim());
    format!("{head}\n{}", body.trim())
}

/// Text → chapters + chunks + raw vectors (iter_rag::prepare), with the
/// model downloaded on first use.
pub fn prepare(title: &str, text: &str) -> Result<Value, String> {
    iter_rag::prepare(embed::get_or_download()?, title, text)
}

pub fn sync(api: &Api, project: &str, topdir: &Path, dry_run: bool, force: bool) -> Result<SyncReport, String> {
    let g = api.get(&format!("/api/projects/{project}/graph")).map_err(|e| format!("reading the map: {e}"))?;
    // iter5 graph: `nodes` (iter4 maps said `vertices`)
    let vertices: Vec<Value> = g.get("nodes").or_else(|| g.get("vertices")).unwrap_or(&Value::Null)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|v| v["path"].as_str().map(|p| p.ends_with(".iter.md")).unwrap_or(false))
        .collect();
    if vertices.is_empty() {
        return Err(format!("the map of '{project}' has no node files — run `iter sync` first"));
    }
    // settings: which node types to index (empty = all) and extra repo globs
    let settings = api.get(&format!("/api/projects/{project}/rag/settings")).unwrap_or(json!({}));
    let strs = |k: &str| -> Vec<String> { settings[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
    let node_types = strs("node_types");
    let vertices: Vec<Value> = vertices.into_iter().filter(|v| node_types.is_empty() || node_types.iter().any(|t| v["nodetype"] == t.as_str())).collect();
    let indexed: Value = if force { json!({}) } else { api.get(&format!("/api/projects/{project}/rag/nodes/hashes")).map_err(|e| e.to_string())? };
    let mut r = SyncReport { total: vertices.len(), ..Default::default() };
    let mut keep = Vec::new();
    let mut changed = Vec::new();
    for v in &vertices {
        let path = v["path"].as_str().unwrap_or("").to_string();
        let text = match std::fs::read_to_string(file_of(topdir, &path)) {
            Ok(t) => t,
            Err(e) => {
                // not in this checkout (yet): keep whatever is indexed
                r.unreadable.push(format!("{path}: {e}"));
                keep.push(path);
                continue;
            }
        };
        keep.push(path.clone());
        let h = sha(&format!("{INDEX_VERSION}\n{text}"));
        if indexed.get(&path).and_then(|x| x.as_str()) == Some(h.as_str()) {
            continue;
        }
        changed.push((v.clone(), path, text, h));
    }
    r.sent = changed.len();
    if dry_run {
        return Ok(r);
    }
    // prepare and send as we go (~1500 chunks per PUT): a large project's
    // vectors never sit in memory all at once
    let total_changed = changed.len();
    let mut batch: Vec<Value> = Vec::new();
    let mut batch_chunks = 0usize;
    let mut done = 0usize;
    let flush = |batch: &mut Vec<Value>, keep: Option<&Vec<String>>, r: &mut SyncReport| -> Result<(), String> {
        let mut body = json!({"nodes": batch});
        if let Some(k) = keep {
            body["keep"] = json!(k);
        }
        let out = put_long(api, &format!("/api/projects/{project}/rag/nodes"), &body)?;
        r.added += out["added"].as_u64().unwrap_or(0);
        r.changed += out["changed"].as_u64().unwrap_or(0);
        r.unchanged += out["unchanged"].as_u64().unwrap_or(0);
        r.removed += out["removed"].as_u64().unwrap_or(0);
        r.failed.extend(out["failed"].as_array().cloned().unwrap_or_default());
        batch.clear();
        Ok(())
    };
    for (v, path, raw, h) in changed {
        let name = v["name"].as_str().unwrap_or("");
        let nodetype = v["nodetype"].as_str().unwrap_or("");
        let title = if name.is_empty() { path.rsplit('/').next().unwrap_or(&path).to_string() } else { name.to_string() };
        let desc = v["desc"].as_str().or_else(|| v["description"].as_str()).unwrap_or("").to_string();
        match prepare(&title, &node_text(name, nodetype, &desc, &raw)) {
            Ok(mut p) => {
                for (k, val) in [("path", json!(path)), ("node_id", v["id"].clone()), ("nodetype", v["nodetype"].clone()), ("name", v["name"].clone()),
                                 ("description", json!(desc)), ("hash", json!(h)), ("format", json!("markdown"))] {
                    p[k] = val;
                }
                batch_chunks += p["chunks"].as_array().map(|a| a.len()).unwrap_or(0);
                batch.push(p);
                done += 1;
                if batch_chunks >= 1500 || batch.len() >= SYNC_BATCH {
                    flush(&mut batch, None, &mut r)?;
                    batch_chunks = 0;
                    if total_changed > SYNC_BATCH {
                        println!("[rag] {project}: {done} of {total_changed} node files indexed so far");
                    }
                }
            }
            Err(e) => r.failed.push(json!({"path": path, "error": e})),
        }
    }
    // the keep list rides on the last call, after every change is in
    flush(&mut batch, Some(&keep), &mut r)?;
    let globs = strs("repo_globs");
    if !globs.is_empty() || !api.get(&format!("/api/projects/{project}/rag/files/hashes")).map(|h| h.as_object().map(|o| o.is_empty()).unwrap_or(true)).unwrap_or(true) {
        sync_repo_files(api, project, topdir, &globs, force, &mut r)?;
    }
    Ok(r)
}

/// Files of the checkout matching `globs` ({topdir}-relative, e.g.
/// `core/fleet_shared/**/*.md`), indexed where they live: only changed ones
/// are extracted, chunked, embedded and sent; files that no longer match drop
/// out. Node files (`*.iter.md`) are left to the node sweep.
fn sync_repo_files(api: &Api, project: &str, topdir: &Path, globs: &[String], force: bool, r: &mut SyncReport) -> Result<(), String> {
    let top = topdir.canonicalize().unwrap_or_else(|_| topdir.to_path_buf());
    let mut paths: std::collections::BTreeSet<std::path::PathBuf> = Default::default();
    for g in globs {
        let rel = g.trim_start_matches("{topdir}").trim_start_matches('/');
        let pattern = top.join(rel).to_string_lossy().into_owned();
        for p in glob::glob(&pattern).map_err(|e| format!("repo_globs {g}: {e}"))?.flatten() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_file() && !name.ends_with(".iter.md") && !p.components().any(|c| c.as_os_str() == ".git") {
                paths.insert(p);
            }
        }
    }
    let indexed: Value = if force { json!({}) } else { api.get(&format!("/api/projects/{project}/rag/files/hashes")).map_err(|e| e.to_string())? };
    let mut keep = Vec::new();
    let mut batch: Vec<Value> = Vec::new();
    let mut batch_chunks = 0usize;
    let send = |batch: &mut Vec<Value>, keep: Option<&Vec<String>>, r: &mut SyncReport| -> Result<(), String> {
        let mut body = json!({"files": batch});
        if let Some(k) = keep {
            body["keep"] = json!(k);
        }
        let out = put_long(api, &format!("/api/projects/{project}/rag/files"), &body)?;
        r.added += out["added"].as_u64().unwrap_or(0);
        r.changed += out["changed"].as_u64().unwrap_or(0);
        r.unchanged += out["unchanged"].as_u64().unwrap_or(0);
        r.removed += out["removed"].as_u64().unwrap_or(0);
        r.failed.extend(out["failed"].as_array().cloned().unwrap_or_default());
        batch.clear();
        Ok(())
    };
    r.total += paths.len();
    for p in &paths {
        let rel = format!("{{topdir}}/{}", p.strip_prefix(&top).unwrap_or(p).to_string_lossy());
        keep.push(rel.clone());
        let bytes = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                r.unreadable.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let h = format!("{}", sha(&format!("{INDEX_VERSION}\n{}", String::from_utf8_lossy(&bytes))));
        if indexed.get(&rel).and_then(|x| x.as_str()) == Some(h.as_str()) {
            continue;
        }
        r.sent += 1;
        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
        let prepared = extract::extract(&fname, &bytes).and_then(|ex| {
            if ex.needs_ocr {
                return Err(format!("{fname}: a scanned PDF — upload it on the GraphRAG tab to have it OCR'd"));
            }
            let title = ex.text.lines().find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string())).filter(|t| !t.is_empty()).unwrap_or_else(|| fname.clone());
            prepare(&title, &ex.text).map(|mut v| {
                v["title"] = json!(title);
                v["format"] = json!(ex.format);
                v["pages"] = json!(ex.pages);
                v
            })
        });
        match prepared {
            Ok(mut v) => {
                v["path"] = json!(rel);
                v["hash"] = json!(h);
                batch_chunks += v["chunks"].as_array().map(|a| a.len()).unwrap_or(0);
                batch.push(v);
                // a PUT carries ~1500 chunks at most (each with its vector)
                if batch_chunks >= 1500 {
                    send(&mut batch, None, r)?;
                    batch_chunks = 0;
                    println!("[rag] {project}: {} of {} repo files indexed so far", keep.len(), paths.len());
                }
            }
            Err(e) => r.failed.push(json!({"path": rel, "error": e})),
        }
    }
    send(&mut batch, Some(&keep), r)
}

/// A PUT that may take longer than the client's 30 s default (embedding a batch).
fn put_long(api: &Api, path: &str, body: &Value) -> Result<Value, String> {
    let http = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(600)).build().map_err(|e| e.to_string())?;
    let resp = http.put(format!("{}{}", api.base, path)).bearer_auth(&api.token).json(body).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{} {path}: {}", status.as_u16(), text.chars().take(400).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

// ---------- 2. the Summary agent ----------

/// Which projects the heartbeat reply says have summary work.
pub fn waiting_projects(reply: &Value) -> Vec<String> {
    reply["rag_waiting"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

pub const SUMMARY_DEFAULT_BODY: &str = "You are iter's Summary agent. You write short retrieval summaries for a semantic search index \
(GraphRAG): a developer or another agent will later search the project's documents in plain words, and your summary is what \
lets them find the right passage. Write for that search:
- say what the passage is about and what it states, in plain declarative sentences — never \"this chunk\", \"this section\" or \"the text\";
- name the concrete things it contains (components, functions, fields, commands, error codes, numbers, people, decisions) exactly as written;
- no opinions, no advice, nothing that is not in the passage.";

/// The prompt for one chunks job (passages may come from several documents).
pub fn chunks_prompt(persona: &str, job: &Value) -> String {
    let mut p = format!(
        "{persona}\n\n# Task\n\nSummarise each passage below. Each names the document it comes from. For each: 1–3 sentences, at most 70 words.\n\
         Answer with JSON only, no prose and no code fence: {{\"summaries\": [{{\"id\": \"<the passage id>\", \"summary\": \"…\"}}]}} — one entry per passage, ids exactly as given.\n\n"
    );
    let fallback = &job["doc"];
    for c in job["chunks"].as_array().into_iter().flatten() {
        let pick = |k: &str| c[k].as_str().filter(|x| !x.is_empty()).or_else(|| fallback[k].as_str()).unwrap_or("").to_string();
        let nt = pick("nodetype");
        p.push_str(&format!(
            "<passage id=\"{}\" document=\"{}\" kind=\"{}{}\" path=\"{}\" heading=\"{}\">\n{}\n</passage>\n\n",
            c["id"].as_str().unwrap_or(""),
            pick("title").replace('"', "'"),
            pick("kind"),
            if nt.is_empty() { String::new() } else { format!(" {nt} node") },
            pick("path"),
            c["heading"].as_str().unwrap_or("").replace('"', "'"),
            c["text"].as_str().unwrap_or("")
        ));
    }
    p
}

/// The documents of a rollup job (`docs`, or the single `doc` + `chapters` of older servers).
pub fn rollup_docs(job: &Value) -> Vec<Value> {
    match job["docs"].as_array() {
        Some(d) => d.clone(),
        None => {
            let mut d = job["doc"].clone();
            d["chapters"] = job["chapters"].clone();
            vec![d]
        }
    }
}

/// The prompt for one rollup job (chapter + document summaries, one or more documents).
pub fn rollup_prompt(persona: &str, job: &Value) -> String {
    let mut p = format!(
        "{persona}\n\n# Task\n\nBelow are the passage summaries of one or more documents, grouped by chapter. For each document write:\n\
         - for each chapter, a summary of 2–4 sentences (at most 90 words);\n\
         - for the whole document, a summary of 3–6 sentences (at most 150 words): what the document is, what it covers, and its key facts or decisions.\n\
         Answer with JSON only, no prose and no code fence: {{\"docs\": [{{\"doc\": \"<the document id>\", \"doc_summary\": \"…\", \"chapters\": [{{\"idx\": 0, \"summary\": \"…\"}}]}}]}} — one entry per document; ids and chapter idx exactly as given.\n\n"
    );
    for d in rollup_docs(job) {
        p.push_str(&format!(
            "<document id=\"{}\" title=\"{}\" kind=\"{}\" path=\"{}\">\n",
            d["id"].as_str().unwrap_or(""),
            d["title"].as_str().unwrap_or("").replace('"', "'"),
            d["kind"].as_str().unwrap_or(""),
            d["path"].as_str().unwrap_or(""),
        ));
        for ch in d["chapters"].as_array().into_iter().flatten() {
            let title = ch["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("(untitled)");
            p.push_str(&format!("<chapter idx=\"{}\" title=\"{}\">\n", ch["idx"], title.replace('"', "'")));
            for s in ch["summaries"].as_array().into_iter().flatten() {
                let h = s["heading"].as_str().unwrap_or("");
                if h.is_empty() {
                    p.push_str(&format!("- {}\n", s["summary"].as_str().unwrap_or("")));
                } else {
                    p.push_str(&format!("- [{h}] {}\n", s["summary"].as_str().unwrap_or("")));
                }
            }
            p.push_str("</chapter>\n");
        }
        p.push_str("</document>\n\n");
    }
    p
}

/// Characters of summary text a document brings to a rollup.
fn doc_text_len(d: &Value) -> usize {
    d["chapters"].as_array().into_iter().flatten().map(|ch| {
        ch["title"].as_str().map(str::len).unwrap_or(0) + 40
            + ch["summaries"].as_array().into_iter().flatten().map(|s| s["summary"].as_str().map(str::len).unwrap_or(0) + s["heading"].as_str().map(str::len).unwrap_or(0) + 8).sum::<usize>()
    }).sum()
}

/// A Summary agent call: no tools, no session on disk — a pure text-in,
/// JSON-out call, run from the temp dir, not the checkout (Claude Code would
/// otherwise load the project's CLAUDE.md into every summary call).
fn summary_ctx(prompt: &str) -> crate::provider::AgentContext {
    crate::provider::AgentContext {
        prompt: prompt.to_string(),
        cwd: std::env::temp_dir(),
        extra_args: vec!["--tools".to_string(), String::new(), "--no-session-persistence".to_string()],
        role: crate::provider::Role::Summary,
        ..Default::default()
    }
}

/// One model call, its spend recorded; the answer's JSON, or why not.
fn ask_model(api: &Api, project: &Project, account: &str, model: &str, timeout: u64, prompt: &str) -> Result<Value, String> {
    let out = crate::provider::call(project, account, model, &summary_ctx(prompt), timeout, Some(2))?;
    if out.cost_usd > 0.0 || out.input_tokens > 0 {
        let _ = api.post(
            &format!("/api/projects/{}/spend", project.key()),
            &json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                    "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens, "workid": "", "agent": "summary"}),
        );
    }
    if out.subtype != "success" {
        return Err(format!("the Summary agent's session ended with '{}'", out.subtype));
    }
    parse_json_answer(&out.text).ok_or_else(|| format!("the Summary agent's answer was not JSON: {}", out.text.chars().take(200).collect::<String>()))
}

/// Chapter windows of at most ROLLUP_WINDOW_CHARS (a chapter never splits).
pub fn chapter_windows(chapters: &[Value]) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    let mut cur: Vec<Value> = Vec::new();
    let mut n = 0usize;
    for ch in chapters {
        let len = doc_text_len(&json!({"chapters": [ch]}));
        if !cur.is_empty() && n + len > ROLLUP_WINDOW_CHARS {
            out.push(std::mem::take(&mut cur));
            n = 0;
        }
        n += len;
        cur.push(ch.clone());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Roll up one document that fits no single prompt: its chapters in windows
/// (chapter summaries from their chunk summaries), then the document from its
/// chapter summaries. Returns a `docs[]` entry of the done report.
fn rollup_big(persona: &str, d: &Value, ask: &dyn Fn(&str) -> Result<Value, String>) -> Result<Value, String> {
    let id = d["id"].clone();
    if doc_text_len(d) <= ROLLUP_WINDOW_CHARS {
        let a = ask(&rollup_prompt(persona, &json!({"docs": [d]})))?;
        let entry = a["docs"].as_array().and_then(|x| x.first()).cloned()
            .or_else(|| a["doc_summary"].is_string().then(|| json!({"doc_summary": a["doc_summary"], "chapters": a["chapters"]})))
            .ok_or("no document summary in the answer")?;
        return Ok(json!({"doc": id, "doc_summary": entry["doc_summary"], "chapters": entry["chapters"]}));
    }
    let chapters = d["chapters"].as_array().cloned().unwrap_or_default();
    let mut chapter_summaries: Vec<Value> = Vec::new();
    for w in chapter_windows(&chapters) {
        let mut p = format!(
            "{persona}\n\n# Task\n\nBelow are passage summaries from part of the document \"{}\" ({}), grouped by chapter. For each chapter write a summary of 2–4 sentences (at most 90 words).\n\
             Answer with JSON only, no prose and no code fence: {{\"chapters\": [{{\"idx\": 0, \"summary\": \"…\"}}]}} — one entry per chapter, idx exactly as given.\n\n",
            d["title"].as_str().unwrap_or(""), d["path"].as_str().unwrap_or("")
        );
        for ch in &w {
            p.push_str(&format!("<chapter idx=\"{}\" title=\"{}\">\n", ch["idx"], ch["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("(untitled)").replace('"', "'")));
            for s in ch["summaries"].as_array().into_iter().flatten() {
                p.push_str(&format!("- {}\n", s["summary"].as_str().unwrap_or("")));
            }
            p.push_str("</chapter>\n");
        }
        let a = ask(&p)?;
        chapter_summaries.extend(a["chapters"].as_array().cloned().unwrap_or_default());
    }
    // the document from its chapter summaries (sampled evenly if even those are too long)
    let titles: std::collections::HashMap<u64, String> = chapters.iter().filter_map(|c| Some((c["idx"].as_u64()?, c["title"].as_str().unwrap_or("").to_string()))).collect();
    let lines: Vec<String> = chapter_summaries.iter().filter_map(|c| {
        let i = c["idx"].as_u64()?;
        Some(format!("- [{}] {}", titles.get(&i).map(String::as_str).unwrap_or(""), c["summary"].as_str()?))
    }).collect();
    let total: usize = lines.iter().map(String::len).sum();
    let step = (total / ROLLUP_WINDOW_CHARS).max(1);
    let body: String = lines.iter().step_by(step).map(|l| format!("{l}\n")).collect();
    let p = format!(
        "{persona}\n\n# Task\n\nBelow are the chapter summaries of the document \"{}\" ({}). Write a summary of the whole document in 3–6 sentences (at most 150 words): what it is, what it covers, and its key facts or decisions.\n\
         Answer with JSON only, no prose and no code fence: {{\"doc_summary\": \"…\"}}\n\n{body}",
        d["title"].as_str().unwrap_or(""), d["path"].as_str().unwrap_or("")
    );
    let a = ask(&p)?;
    let sm = a["doc_summary"].as_str().ok_or("no document summary in the answer")?;
    Ok(json!({"doc": id, "doc_summary": sm, "chapters": chapter_summaries}))
}

/// The first JSON object in a model answer (tolerates a code fence or a
/// preamble). An answer that is not valid JSON as a whole (a stray quote in
/// one summary, a cut-off tail) still yields every `{"id": …, "summary": …}`
/// pair that parses on its own, so one bad summary does not cost the batch.
pub fn parse_json_answer(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    if let (Some(start), Some(end)) = (t.find('{'), t.rfind('}')) {
        if start < end {
            if let Ok(v) = serde_json::from_str::<Value>(&t[start..=end]) {
                return Some(v);
            }
        }
    }
    let pairs = salvage_pairs(t);
    (!pairs.is_empty()).then(|| json!({"summaries": pairs, "salvaged": true}))
}

/// Every innermost `{…}` in `text` that parses and carries both an id and a summary.
fn salvage_pairs(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let (mut in_str, mut esc) = (false, false);
    for (i, ch) in text.char_indices() {
        if in_str {
            match (esc, ch) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => starts.push(i),
            '}' => {
                if let Some(st) = starts.pop() {
                    if let Ok(v) = serde_json::from_str::<Value>(&text[st..=i]) {
                        if v["id"].is_string() && v["summary"].is_string() {
                            out.push(v);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Claim-and-summarise until the project has no work or JOBS_PER_RUN is
/// reached. Runs on its own thread; returns how many jobs it finished.
pub fn summarize_waiting(api: &Api, engine: &str, project: &Project, topdir: &str, account: &str, agent_def: &Value) -> usize {
    let model = agent_def["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_MODEL).to_string();
    let timeout = agent_def["timeoutsec"].as_u64().unwrap_or(DEFAULT_TIMEOUT_SEC).clamp(30, 1800);
    let body = agent_def["promptbody"].as_str().unwrap_or("");
    let persona = if body.trim().lines().count() > 3 { body.to_string() } else { SUMMARY_DEFAULT_BODY.to_string() };
    let base = format!("/api/projects/{}/rag/work", project.key());
    let mut done = 0;
    for _ in 0..JOBS_PER_RUN {
        let batch = agent_def["batch"].as_u64().unwrap_or(DEFAULT_BATCH).clamp(1, 32);
        let job = match api.post(&format!("{base}/claim"), &json!({"engine": engine, "max_chunks": batch})) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[engine] {}: GraphRAG claim failed: {e}", project.key());
                break;
            }
        };
        let kind = job["job"].as_str().unwrap_or("").to_string();
        if kind.is_empty() {
            break;
        }
        if kind == "ingest" {
            ingest(api, engine, project, account, &model, &job);
            done += 1;
            continue;
        }
        let started = std::time::Instant::now();
        let docs_in = rollup_docs(&job);
        if kind == "rollup" && docs_in.iter().map(doc_text_len).sum::<usize>() > ROLLUP_WINDOW_CHARS {
            // too much for one prompt: each document on its own, big ones by chapter windows
            let ask = |prompt: &str| ask_model(api, project, account, &model, timeout, prompt);
            let mut docs = Vec::new();
            let mut errors = Vec::new();
            for d in &docs_in {
                match rollup_big(&persona, d, &ask) {
                    Ok(entry) => docs.push(entry),
                    Err(e) => errors.push(format!("{}: {e}", d["title"].as_str().unwrap_or(""))),
                }
            }
            let embedder = embed::get_or_download();
            for entry in docs.iter_mut() {
                let title = docs_in.iter().find(|d| d["id"] == entry["doc"]).and_then(|d| d["title"].as_str()).unwrap_or("").to_string();
                if let (Ok(e), Some(sm)) = (&embedder, entry["doc_summary"].as_str()) {
                    entry["vec_sum"] = e.embed(&[format!("{title} — {}", sm.trim())]).ok().and_then(|mut v| v.pop()).map(|v| json!(v)).unwrap_or(Value::Null);
                }
            }
            let mut report = json!({"engine": engine, "job": "rollup", "job_id": job["job_id"], "model": model, "docs": docs, "error": errors.join("; ")});
            if let Some(sc) = job["scope"].as_str() {
                report["scope"] = json!(sc);
            }
            if let Ok(e) = &embedder {
                report["embed_model"] = json!(e.stamp);
            }
            let titles = docs_in.iter().filter_map(|d| d["title"].as_str()).collect::<Vec<_>>().join(", ");
            match api.post(&format!("{base}/done"), &report) {
                Ok(r) => println!("[engine] {}: GraphRAG rollup (by chapter windows) '{titles}' in {}s ({model}) — {r}", project.key(), started.elapsed().as_secs()),
                Err(e) => eprintln!("[engine] {}: GraphRAG could not report job {}: {e}", project.key(), job["job_id"]),
            }
            done += 1;
            continue;
        }
        let prompt = if kind == "chunks" { chunks_prompt(&persona, &job) } else { rollup_prompt(&persona, &job) };
        // no tools, no session on disk: a pure text-in, JSON-out call
        let _ = topdir;
        let run = crate::provider::call(project, account, &model, &summary_ctx(&prompt), timeout, Some(2));
        let (answer, error) = match &run {
            Ok(out) if out.subtype == "success" => match parse_json_answer(&out.text) {
                Some(v) => (v, String::new()),
                None => (Value::Null, format!("the Summary agent's answer was not JSON: {}", out.text.chars().take(300).collect::<String>())),
            },
            Ok(out) => (Value::Null, format!("the Summary agent's session ended with '{}'", out.subtype)),
            Err(e) => (Value::Null, e.chars().take(500).collect()),
        };
        let mut report = json!({"engine": engine, "job": kind, "job_id": job["job_id"], "model": model, "error": error});
        // product-wide work (the built-in user guide) is reported to its own scope
        if let Some(sc) = job["scope"].as_str() {
            report["scope"] = json!(sc);
        }
        // the summary vectors are made here, with the same model as the chunks'
        let embedder = embed::get_or_download();
        if let Ok(e) = &embedder {
            report["embed_model"] = json!(e.stamp);
        }
        let embed_one = |text: String| -> Value {
            embedder.as_ref().ok().and_then(|e| e.embed(&[text]).ok()).and_then(|mut v| v.pop()).map(|v| json!(v)).unwrap_or(Value::Null)
        };
        if kind == "chunks" {
            let by_id: std::collections::HashMap<String, &Value> =
                job["chunks"].as_array().into_iter().flatten().filter_map(|c| Some((c["id"].as_str()?.to_string(), c))).collect();
            let mut results = answer["summaries"].as_array().cloned().unwrap_or_default();
            for r in results.iter_mut() {
                let (Some(id), Some(sm)) = (r["id"].as_str(), r["summary"].as_str()) else { continue };
                let Some(c) = by_id.get(id) else { continue };
                let title = c["title"].as_str().unwrap_or("");
                let heading = c["heading"].as_str().unwrap_or("");
                let input = if heading.is_empty() { format!("{title} — {}", sm.trim()) } else { format!("{title} — {heading}: {}", sm.trim()) };
                r["vec_sum"] = embed_one(input);
            }
            report["results"] = json!(results);
        } else {
            // one entry per document; a single-document answer of the old shape is accepted too
            let mut docs = answer["docs"].as_array().cloned().unwrap_or_default();
            if docs.is_empty() && answer["doc_summary"].is_string() {
                if let Some(d) = rollup_docs(&job).first() {
                    docs.push(json!({"doc": d["id"], "doc_summary": answer["doc_summary"], "chapters": answer["chapters"]}));
                }
            }
            let titles: std::collections::HashMap<String, String> = rollup_docs(&job)
                .iter()
                .filter_map(|d| Some((d["id"].as_str()?.to_string(), d["title"].as_str().unwrap_or("").to_string())))
                .collect();
            for d in docs.iter_mut() {
                let (Some(id), Some(sm)) = (d["doc"].as_str(), d["doc_summary"].as_str()) else { continue };
                if !sm.trim().is_empty() {
                    d["vec_sum"] = embed_one(format!("{} — {}", titles.get(id).map(String::as_str).unwrap_or(""), sm.trim()));
                }
            }
            report["docs"] = json!(docs);
        }
        let title = if kind == "chunks" {
            let mut t: Vec<String> = job["chunks"].as_array().into_iter().flatten().filter_map(|c| c["title"].as_str().map(String::from)).collect();
            t.dedup();
            t.join(", ")
        } else {
            rollup_docs(&job).iter().filter_map(|d| d["title"].as_str().map(String::from)).collect::<Vec<_>>().join(", ")
        };
        match api.post(&format!("{base}/done"), &report) {
            Ok(r) => println!(
                "[engine] {}: GraphRAG {kind} '{}' summarised in {}s ({model}){}",
                project.key(),
                title,
                started.elapsed().as_secs(),
                if error.is_empty() { format!(" — {}", r) } else { format!(" — FAILED: {error}") }
            ),
            Err(e) => eprintln!("[engine] {}: GraphRAG could not report job {}: {e}", project.key(), job["job_id"]),
        }
        if let Ok(out) = &run {
            if out.cost_usd > 0.0 || out.input_tokens > 0 {
                let _ = api.post(
                    &format!("/api/projects/{}/spend", project.key()),
                    &json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                            "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens,
                            "workid": "", "agent": "summary"}),
                );
            }
        }
        done += 1;
        if !error.is_empty() {
            break; // a failing model is not hammered: the next tick tries again
        }
    }
    done
}

// ---------- ingest (uploads) ----------

/// One ingest job: extract (OCR a scan), prepare, PUT the chunks — or report
/// why not (a file that cannot be read is failed for good, not retried).
fn ingest(api: &Api, engine: &str, project: &Project, account: &str, model: &str, job: &Value) {
    use base64::Engine as _;
    let started = std::time::Instant::now();
    let doc = &job["doc"];
    let id = doc["id"].as_str().unwrap_or("").to_string();
    let title = doc["title"].as_str().unwrap_or("").to_string();
    let filename = job["filename"].as_str().unwrap_or("document").to_string();
    let run = || -> Result<Value, String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(job["content_b64"].as_str().unwrap_or("")).map_err(|e| format!("content_b64: {e}"))?;
        let mut ex = extract::extract(&filename, &bytes)?;
        let mut ocr = false;
        if ex.needs_ocr {
            println!("[engine] {}: GraphRAG '{title}' is a scanned PDF ({} pages) — reading it with {model}", project.key(), ex.pages);
            ex.text = extract::tidy(&ocr_pdf(project, account, model, &bytes, ex.pages)?);
            ocr = true;
            if ex.text.trim().is_empty() {
                return Err(format!("{filename}: no text could be extracted, even by OCR"));
            }
        }
        let mut p = prepare(&title, &ex.text)?;
        p["format"] = json!(ex.format);
        p["pages"] = json!(ex.pages);
        p["ocr"] = json!(ocr);
        p["engine"] = json!(engine);
        p["job_id"] = job["job_id"].clone();
        put_long(api, &format!("/api/projects/{}/rag/docs/{id}/chunks", project.key()), &p)
    };
    match run() {
        Ok(d) => println!(
            "[engine] {}: GraphRAG ingested '{title}' — {} chunks in {} chapters, {}s",
            project.key(), d["chunks"], d["chapters"].as_array().map(|c| c.len()).unwrap_or(0), started.elapsed().as_secs()
        ),
        Err(e) => {
            println!("[engine] {}: GraphRAG could not ingest '{title}': {e}", project.key());
            let _ = api.post(
                &format!("/api/projects/{}/rag/work/done", project.key()),
                &json!({"engine": engine, "job": "ingest", "job_id": job["job_id"], "doc": id, "error": e}),
            );
        }
    }
}

/// OCR a scanned PDF: render the pages to PNG with poppler's `pdftoppm`, then
/// have a Claude session (the Summary agent's model; Read is its only tool,
/// confined to a scratch directory) transcribe them, OCR_PAGES_PER_CALL at a
/// time, as markdown with a `# Page N` heading per page.
pub fn ocr_pdf(project: &Project, account: &str, model: &str, bytes: &[u8], pages: usize) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("iter-ocr-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let cleanup = scopeguard(dir.clone());
    std::fs::write(dir.join("doc.pdf"), bytes).map_err(|e| e.to_string())?;
    let last = pages.clamp(1, OCR_MAX_PAGES);
    let out = std::process::Command::new("pdftoppm")
        .args(["-r", "110", "-png", "-f", "1", "-l", &last.to_string(), "doc.pdf", "page"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("OCR needs poppler's pdftoppm on the engine machine (brew install poppler / apt install poppler-utils): {e}"))?;
    if !out.status.success() {
        return Err(format!("pdftoppm failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut imgs: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("page") && n.ends_with(".png"))
        .collect();
    imgs.sort();
    let mut text = String::new();
    for (bi, batch) in imgs.chunks(OCR_PAGES_PER_CALL).enumerate() {
        let first = bi * OCR_PAGES_PER_CALL + 1;
        let list: String = batch.iter().enumerate().map(|(i, f)| format!("- page {}: {f}\n", first + i)).collect();
        let prompt = format!(
            "Transcribe scanned document pages. Read each image file listed below with the Read tool (they are in the current directory), \
             in order, and write out ALL of the text on each page, faithfully, as markdown: start each page with a line `# Page N`, keep headings as \
             `##` lines, lists as lists and tables as markdown tables. Write nothing else: no commentary, no summary.\n\n{list}"
        );
        let ctx = crate::provider::AgentContext {
            prompt,
            cwd: dir.clone(),
            allowed_tools: Some("Read".to_string()),
            extra_args: vec!["--no-session-persistence".to_string()],
            role: crate::provider::Role::Ocr,
            ..Default::default()
        };
        let res = crate::provider::call(project, account, model, &ctx, 900, Some((batch.len() * 2 + 4) as u32))?;
        if res.subtype != "success" {
            return Err(format!("OCR of pages {first}–{} ended with '{}'", first + batch.len() - 1, res.subtype));
        }
        text.push_str(res.text.trim());
        text.push_str("\n\n");
    }
    if pages > OCR_MAX_PAGES {
        text.push_str(&format!("\n\n(Pages {}–{pages} were not transcribed: OCR stops at {OCR_MAX_PAGES} pages.)\n", OCR_MAX_PAGES + 1));
    }
    drop(cleanup);
    Ok(text)
}

/// Removes a scratch directory when dropped (also on early returns).
struct DirGuard(std::path::PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scopeguard(p: std::path::PathBuf) -> DirGuard {
    DirGuard(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_carry_ids_and_the_json_contract() {
        let job = json!({"job": "chunks", "doc": {"title": "Design", "kind": "file", "path": "{topdir}/docs/d.md"},
                         "chunks": [{"id": "fabc_00000", "heading": "Intro \"x\"", "text": "Alpha."}, {"id": "fabc_00001", "heading": "", "text": "Beta."}]});
        let p = chunks_prompt("PERSONA", &job);
        assert!(p.starts_with("PERSONA"));
        assert!(p.contains("<passage id=\"fabc_00000\" document=\"Design\" kind=\"file\" path=\"{topdir}/docs/d.md\" heading=\"Intro 'x'\">\nAlpha.") && p.contains("fabc_00001"));
        assert!(p.contains("\"summaries\""));
        let r = rollup_prompt("P", &json!({"docs": [{"id": "d1", "title": "D", "chapters": [{"idx": 0, "title": "", "summaries": [{"heading": "A", "summary": "s1"}]}]}]}));
        assert!(r.contains("<document id=\"d1\" title=\"D\"") && r.contains("<chapter idx=\"0\" title=\"(untitled)\">\n- [A] s1"));
        // an older server's single-document job
        assert_eq!(rollup_docs(&json!({"doc": {"id": "x"}, "chapters": []}))[0]["id"], "x");
    }

    #[test]
    fn big_documents_roll_up_in_chapter_windows() {
        let ch = |i: u64, n: usize| json!({"idx": i, "title": format!("c{i}"), "summaries": (0..n).map(|_| json!({"heading": "", "summary": "s".repeat(990)})).collect::<Vec<_>>()});
        let chapters: Vec<Value> = (0..10).map(|i| ch(i, 15)).collect(); // ~15k chars each
        let w = chapter_windows(&chapters);
        assert_eq!(w.len(), 5, "two ~15k chapters per 40k window");
        assert_eq!(w.iter().map(|x| x.len()).sum::<usize>(), 10);
        // a scripted model: chapter windows, then the document
        let calls = std::cell::RefCell::new(0);
        let ask = |p: &str| -> Result<Value, String> {
            *calls.borrow_mut() += 1;
            if p.contains("\"chapters\": [") && p.contains("<chapter idx=") {
                let ids: Vec<u64> = p.match_indices("<chapter idx=\"").map(|(i, _)| p[i + 14..].split('"').next().unwrap().parse().unwrap()).collect();
                Ok(json!({"chapters": ids.iter().map(|i| json!({"idx": i, "summary": format!("chapter {i}")})).collect::<Vec<_>>()}))
            } else {
                Ok(json!({"doc_summary": "the whole"}))
            }
        };
        let d = json!({"id": "d1", "title": "Big", "path": "{topdir}/big.md", "chapters": chapters});
        let entry = rollup_big("P", &d, &ask).unwrap();
        assert_eq!(entry["doc_summary"], "the whole");
        assert_eq!(entry["chapters"].as_array().unwrap().len(), 10);
        assert_eq!(*calls.borrow(), 6, "5 windows + 1 document call");
    }

    #[test]
    fn node_text_drops_frontmatter_keeps_description() {
        let t = node_text("Engine", "code", "Runs agents.", "---\nid: x\nname: Engine\n---\n\n# Long Description\n\nBody.");
        assert!(t.starts_with("# Engine (code)\n\nRuns agents."));
        assert!(t.contains("Body.") && !t.contains("id: x"));
    }

    #[test]
    fn json_answers_survive_fences_and_preambles() {
        assert_eq!(parse_json_answer("{\"a\":1}").unwrap()["a"], 1);
        assert_eq!(parse_json_answer("Here you go:\n```json\n{\"a\": {\"b\": 2}}\n```").unwrap()["a"]["b"], 2);
        assert!(parse_json_answer("no json here").is_none());
        // one broken summary does not cost the others
        let broken = r#"{"summaries": [{"id": "a_1", "summary": "fine"}, {"id": "a_2", "summary": "says "hi" badly"}, {"id": "a_3", "summary": "also fine"}]}"#;
        let got = parse_json_answer(broken).unwrap();
        let ids: Vec<&str> = got["summaries"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["a_1", "a_3"]);
        assert_eq!(waiting_projects(&json!({"rag_waiting": {"p": 3}})), vec!["p".to_string()]);
    }
}
