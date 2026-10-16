//! GraphRAG's shared text and embedding code (iter4, 2026-09-29): file → text
//! (`extract`), text → chapters + chunks sized for the model (`chunk`), and
//! the all-MiniLM-L6-v2 embedder (`embed`). The engine runs all three for
//! bulk work; iter_data runs only the embedder, for search questions.

pub mod chunk;
pub mod embed;
pub mod extract;

use serde_json::{Value, json};

/// Text → chapters + chunks sized in the model's tokens + one raw-text vector
/// per chunk, in the shape iter_data stores (`Prepared`). The engine calls it
/// for uploads and node files; iter_data for the built-in user guide.
pub fn prepare(e: &embed::Embedder, title: &str, text: &str) -> Result<Value, String> {
    let count = |s: &str| e.count_tokens(s);
    let sizer = chunk::Sizer::tokens(&count, embed::MAX_TOKENS);
    let (chapters, chunks) = chunk::chunk_with(text, title, &sizer);
    if chunks.is_empty() {
        return Err(format!("{title}: nothing to index after chunking"));
    }
    let inputs: Vec<String> = chunks.iter().map(|c| chunk::embed_input(title, &c.heading, &c.text)).collect();
    let vecs = e.embed(&inputs)?;
    Ok(json!({
        "model": e.stamp, "chars": text.len(),
        "chapters": chapters.iter().map(|c| json!({"idx": c.idx, "title": c.title})).collect::<Vec<_>>(),
        "chunks": chunks.iter().zip(vecs).zip(inputs.iter()).map(|((c, v), inp)| json!({
            "idx": c.idx, "chapter": c.chapter, "heading": c.heading, "text": c.text, "tokens": e.count_tokens(inp), "vec_raw": v,
        })).collect::<Vec<_>>(),
    }))
}
