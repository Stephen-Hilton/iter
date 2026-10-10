//! Text → chapters + chunks (GraphRAG, 2026-09-29).
//!
//! - **Chapters** are the sections under the document's top heading level
//!   (markdown `#` lines, which docx/html extraction also produce). Text with
//!   no markdown headings (a PDF, plain text) uses heading-like lines instead
//!   ("Chapter 3 …", "2.1 Scope") when it has at least two. Text before the
//!   first heading is its own chapter. No headings at all: one chapter.
//! - **Chunks** are runs of whole paragraphs (a fenced code block counts as
//!   one) up to a size budget (a longer paragraph is split at sentence, then
//!   line, then word boundaries). A chunk never spans two chapters, and
//!   carries its heading path ("Design > Storage").
//!
//! Sizes are measured by a `Sizer`. The engine measures in the embedding
//! model's own word-pieces (`Sizer::tokens`), so the text the model sees —
//! `embed_input`: "<title> — <heading>" + the chunk — fits its 256-token
//! window whole and no tail is cut off (paths and identifiers cost many
//! word-pieces: 1000 characters of code can be 400). `Sizer::chars`
//! (~1000 characters) is the fallback where no tokenizer is at hand.

pub const TARGET: usize = 1000;
pub const MAX: usize = 1500;

/// How chunk sizes are measured and bounded.
pub struct Sizer<'a> {
    pub measure: &'a dyn Fn(&str) -> usize,
    /// a chunk is closed once it reaches this
    pub target: usize,
    /// and never exceeds this (minus the title/heading prefix when `prefixed`)
    pub max: usize,
    /// the budget includes `embed_input`'s prefix (token sizing)
    pub prefixed: bool,
}

fn char_len(s: &str) -> usize {
    s.len()
}

impl Sizer<'static> {
    pub fn chars() -> Self {
        Sizer { measure: &char_len, target: TARGET, max: MAX, prefixed: false }
    }
}

impl<'a> Sizer<'a> {
    /// Word-piece sizing for a model whose window is `window` tokens ([CLS] and
    /// [SEP] take two; a small margin absorbs counting at chunk joins).
    pub fn tokens(measure: &'a dyn Fn(&str) -> usize, window: usize) -> Self {
        let max = window.saturating_sub(2 + 6);
        Sizer { measure, target: max * 4 / 5, max, prefixed: true }
    }
}

/// The text a chunk is embedded as: its place in the document, then the chunk.
pub fn embed_input(title: &str, heading: &str, text: &str) -> String {
    if heading.is_empty() { format!("{title}\n{text}") } else { format!("{title} — {heading}\n{text}") }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub idx: usize,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub idx: usize,
    pub chapter: usize,
    /// heading path inside the document, outermost first, " > "-joined
    pub heading: String,
    pub text: String,
}

/// Split off a leading `---` YAML frontmatter block: (frontmatter, body).
pub fn split_frontmatter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let after = &rest[end + 4..];
            let body = after.split_once('\n').map(|(_, b)| b).unwrap_or("");
            return (fm, body);
        }
    }
    ("", text)
}

fn md_heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        let title = t[hashes..].trim().trim_end_matches('#').trim().to_string();
        if !title.is_empty() {
            return Some((hashes, title));
        }
    }
    None
}

/// A heading-like line in text with no markdown headings.
fn plain_heading(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 || t.len() > 80 || t.ends_with('.') || t.ends_with(',') || t.ends_with(':') {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    for w in ["chapter ", "part ", "section ", "appendix "] {
        if lower.starts_with(w) {
            return true;
        }
    }
    // "3 Scope", "2.1 Storage", "4.2.1. Locks" — a number then a capitalised word
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if !num.is_empty() && num.chars().next().unwrap().is_ascii_digit() && num.len() <= 8 {
        let rest = t[num.len()..].trim_start();
        return t[num.len()..].starts_with(' ') && rest.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) && rest.split_whitespace().count() <= 10;
    }
    false
}

struct Section {
    level: usize, // 0 = preamble
    title: String,
    lines: Vec<String>,
}

fn sections(text: &str) -> Vec<Section> {
    let lines: Vec<&str> = text.lines().collect();
    let mut in_fence = false;
    let mut has_md = false;
    for l in &lines {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence && md_heading(l).is_some() {
            has_md = true;
            break;
        }
    }
    let use_plain = !has_md && lines.iter().filter(|l| plain_heading(l)).count() >= 2;
    let mut out = vec![Section { level: 0, title: String::new(), lines: vec![] }];
    in_fence = false;
    for l in lines {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let head = if in_fence {
            None
        } else if has_md {
            md_heading(l)
        } else if use_plain && plain_heading(l) {
            Some((1, l.trim().to_string()))
        } else {
            None
        };
        match head {
            Some((lv, title)) => out.push(Section { level: lv, title, lines: vec![] }),
            None => out.last_mut().unwrap().lines.push(l.to_string()),
        }
    }
    out
}

/// Paragraph blocks: blank-line separated; a fenced block stays whole.
fn blocks(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for l in lines {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && l.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n"));
                cur.clear();
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join("\n"));
    }
    out
}

/// Split after sentence ends and line breaks, keeping the separators.
fn units(block: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let b = block.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let end = if b[i] == b'\n' {
            Some(i + 1)
        } else if matches!(b[i], b'.' | b'?' | b'!') && b.get(i + 1) == Some(&b' ') {
            Some(i + 2)
        } else {
            None
        };
        if let Some(e) = end {
            out.push(&block[start..e]);
            start = e;
            i = e;
        } else {
            i += 1;
        }
    }
    if start < block.len() {
        out.push(&block[start..]);
    }
    out
}

/// Cut one oversized block to pieces within `max`: at sentence ends and line
/// breaks, and a sentence that alone is too big at spaces.
fn split_long(block: &str, sz: &Sizer, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut n) = (String::new(), 0usize);
    let push = |piece: &str, size: usize, cur: &mut String, n: &mut usize, out: &mut Vec<String>| {
        if !cur.is_empty() && *n + size > max {
            out.push(cur.trim().to_string());
            cur.clear();
            *n = 0;
        }
        cur.push_str(piece);
        *n += size;
    };
    for u in units(block) {
        let size = (sz.measure)(u);
        if size <= max {
            push(u, size, &mut cur, &mut n, &mut out);
            continue;
        }
        for w in u.split_inclusive(' ') {
            let ws = (sz.measure)(w).max(1);
            push(w, ws, &mut cur, &mut n, &mut out);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|p| !p.is_empty());
    out
}

pub fn chunk(text: &str) -> (Vec<Chapter>, Vec<Chunk>) {
    chunk_with(text, "", &Sizer::chars())
}

/// Chapters and chunks of `text`, sized by `sz`; `title` is the document's
/// (it is part of every chunk's embedding input, so part of the budget).
pub fn chunk_with(text: &str, title: &str, sz: &Sizer) -> (Vec<Chapter>, Vec<Chunk>) {
    let secs = sections(text);
    // a lone heading at the top level is the document's title, not a chapter:
    // the chapters are the next level down (a markdown file's "# Title" + "## …")
    let mut top = secs.iter().filter(|s| s.level > 0).map(|s| s.level).min().unwrap_or(0);
    while top > 0 && secs.iter().filter(|s| s.level == top).count() == 1 {
        match secs.iter().filter(|s| s.level > top).map(|s| s.level).min() {
            Some(next) => top = next,
            None => break,
        }
    }
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut chunks: Vec<Chunk> = Vec::new();
    // heading stack (level, title) for the path
    let mut stack: Vec<(usize, String)> = Vec::new();
    for sec in &secs {
        if sec.level > 0 {
            while stack.last().map(|(l, _)| *l >= sec.level).unwrap_or(false) {
                stack.pop();
            }
            stack.push((sec.level, sec.title.clone()));
        }
        let starts_chapter = chapters.is_empty() || (sec.level > 0 && sec.level <= top);
        let body_blocks = blocks(&sec.lines);
        if starts_chapter {
            if sec.level == 0 && body_blocks.is_empty() {
                continue; // no preamble
            }
            let title = if sec.level == 0 {
                if top == 0 { String::new() } else { "(opening)".to_string() }
            } else {
                sec.title.clone()
            };
            chapters.push(Chapter { idx: chapters.len(), title });
        }
        let ch = chapters.len() - 1;
        let heading = stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" > ");
        // this section's budget: the window minus the prefix every chunk here carries
        let max = if sz.prefixed { sz.max.saturating_sub((sz.measure)(&embed_input(title, &heading, ""))).max(sz.max / 3) } else { sz.max };
        let target = (max * 4 / 5).min(sz.target).max(1);
        let mut cur = String::new();
        let mut n = 0usize;
        let flush = |cur: &mut String, n: &mut usize, chunks: &mut Vec<Chunk>| {
            if !cur.trim().is_empty() {
                chunks.push(Chunk { idx: chunks.len(), chapter: ch, heading: heading.clone(), text: cur.trim().to_string() });
            }
            cur.clear();
            *n = 0;
        };
        let sep = (sz.measure)("\n\n").max(if sz.prefixed { 0 } else { 2 });
        for b in body_blocks {
            let bs = (sz.measure)(&b);
            let pieces: Vec<(String, usize)> = if bs > max {
                split_long(&b, sz, max).into_iter().map(|p| { let m = (sz.measure)(&p); (p, m) }).collect()
            } else {
                vec![(b, bs)]
            };
            for (p, ps) in pieces {
                if !cur.is_empty() && n + sep + ps > max {
                    flush(&mut cur, &mut n, &mut chunks);
                }
                if !cur.is_empty() {
                    cur.push_str("\n\n");
                    n += sep;
                }
                cur.push_str(&p);
                n += ps;
                if n >= target {
                    flush(&mut cur, &mut n, &mut chunks);
                }
            }
        }
        flush(&mut cur, &mut n, &mut chunks);
    }
    // drop chapters that ended with no chunk (a heading with nothing under it)
    let used: std::collections::BTreeSet<usize> = chunks.iter().map(|c| c.chapter).collect();
    let remap: std::collections::HashMap<usize, usize> = used.iter().enumerate().map(|(n, o)| (*o, n)).collect();
    let chapters: Vec<Chapter> = chapters
        .into_iter()
        .filter(|c| used.contains(&c.idx))
        .map(|c| Chapter { idx: remap[&c.idx], title: c.title })
        .collect();
    for c in chunks.iter_mut() {
        c.chapter = remap[&c.chapter];
    }
    (chapters, chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_follow_the_top_heading_level() {
        let t = "intro para\n\n# One\n\nalpha\n\n## One.a\n\nbeta\n\n# Two\n\ngamma\n\n# Empty\n";
        let (ch, ck) = chunk(t);
        assert_eq!(ch.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["(opening)", "One", "Two"]);
        assert_eq!(ck.len(), 4);
        assert_eq!((ck[2].chapter, ck[2].heading.as_str(), ck[2].text.as_str()), (1, "One > One.a", "beta"));
        assert_eq!(ck[3].chapter, 2);
    }

    #[test]
    fn plain_text_headings_and_sizes() {
        let para = "word ".repeat(150); // 750 chars
        let t = format!("Chapter 1 Start\n\n{para}\n\n{para}\n\n2.1 Next Part\n\n{}", "x. ".repeat(1200));
        let (ch, ck) = chunk(&t);
        assert_eq!(ch.len(), 2);
        assert!(ck.iter().all(|c| c.text.len() <= MAX), "{:?}", ck.iter().map(|c| c.text.len()).collect::<Vec<_>>());
        assert!(ck.iter().filter(|c| c.chapter == 0).count() == 2);
    }

    #[test]
    fn code_fences_hide_hashes_and_frontmatter_splits() {
        let (fm, body) = split_frontmatter("---\nid: x\nname: y\n---\n\n# Body\n\n```\n# not a heading\n```\n");
        assert_eq!(fm, "id: x\nname: y");
        let (ch, ck) = chunk(body);
        assert_eq!(ch.len(), 1);
        assert!(ck[0].text.contains("# not a heading"));
    }

    #[test]
    fn a_lone_title_is_not_the_only_chapter() {
        let (ch, _) = chunk("# Spec\n\nintro\n\n## One\n\na\n\n## Two\n\nb\n\n### Two.a\n\nc\n");
        assert_eq!(ch.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["Spec", "One", "Two"]);
    }

    #[test]
    fn token_sized_chunks_fit_the_model_window_whole() {
        let Some(dir) = crate::embed::find_model_dir() else { return };
        let e = crate::embed::Embedder::load(&dir).unwrap();
        let count = |s: &str| e.count_tokens(s);
        let sz = Sizer::tokens(&count, crate::embed::MAX_TOKENS);
        // code-heavy text: short in characters, long in word-pieces
        let line = "- `{topdir}/iter_data/src/rag/mod.rs: work_claim` claims `rag_chunk` rows (sum_state=claimed).\n";
        let text = format!("# Doc\n\n## Paths\n\n{}\n\n## Prose\n\n{}", line.repeat(40), "The engine runs the agent. ".repeat(120));
        let (_, ck) = chunk_with(&text, "iter4 spec", &sz);
        assert!(ck.len() > 4);
        for c in &ck {
            let n = e.count_tokens(&embed_input("iter4 spec", &c.heading, &c.text));
            assert!(n + 2 <= crate::embed::MAX_TOKENS, "chunk {} is {n} tokens: {}", c.idx, c.text);
        }
    }

    #[test]
    fn no_headings_one_chapter() {
        let (ch, ck) = chunk("just some text\n\nand more");
        assert_eq!((ch.len(), ch[0].title.as_str(), ck.len()), (1, "", 1));
    }
}
