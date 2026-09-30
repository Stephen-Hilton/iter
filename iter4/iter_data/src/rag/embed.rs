//! The embedding model (GraphRAG, 2026-09-29): sentence-transformers
//! all-MiniLM-L6-v2 run in-process with candle (pure Rust, CPU) — 6 BERT
//! layers, 384-dim output, mean-pooled over the attention mask and
//! L2-normalised, so cosine similarity is a dot product.
//!
//! Lives in iter_data (not the engine) because a search embeds its query at
//! request time and iter_data is what answers searches.  The model directory
//! is found once, lazily (a server that never ingests never loads it):
//! `--embed-model` / `$ITER_EMBED_MODEL`, else `models/all-MiniLM-L6-v2`
//! beside the working directory or the binary, else the container's
//! `/opt/iter/models/all-MiniLM-L6-v2`.
//!
//! Input past MAX_TOKENS word-pieces is cut off (the model was trained on
//! 256); chunks are sized to fit (chunk.rs), and the LLM summary vector
//! covers what a long chunk's raw vector cannot.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const MODEL_NAME: &str = "all-MiniLM-L6-v2";
pub const DIM: usize = 384;
pub const MAX_TOKENS: usize = 256;
/// texts per forward pass
const BATCH: usize = 16;

pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    pub dir: PathBuf,
}

static MODEL_DIR_FLAG: OnceLock<String> = OnceLock::new();
static EMBEDDER: OnceLock<Result<Embedder, String>> = OnceLock::new();

/// `--embed-model` from main (before the first use).
pub fn set_model_dir(dir: &str) {
    if !dir.is_empty() {
        let _ = MODEL_DIR_FLAG.set(dir.to_string());
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = MODEL_DIR_FLAG.get() {
        v.push(PathBuf::from(d));
    }
    if let Ok(d) = std::env::var("ITER_EMBED_MODEL") {
        if !d.is_empty() {
            v.push(PathBuf::from(d));
        }
    }
    let rel = Path::new("models").join(MODEL_NAME);
    v.push(rel.clone());
    if let Ok(exe) = std::env::current_exe() {
        // target/release/iter_data → iter4/models/…
        for anc in exe.ancestors().skip(1).take(4) {
            v.push(anc.join(&rel));
        }
    }
    v.push(PathBuf::from("/opt/iter/models").join(MODEL_NAME));
    v
}

pub fn find_model_dir() -> Option<PathBuf> {
    candidates().into_iter().find(|d| d.join("model.safetensors").is_file() && d.join("tokenizer.json").is_file())
}

/// The process-wide model, loaded on first use.
pub fn get() -> Result<&'static Embedder, String> {
    EMBEDDER
        .get_or_init(|| {
            let dir = find_model_dir().ok_or_else(|| {
                format!("embedding model {MODEL_NAME} not found (looked in {:?}); run iter4/tools/fetch_model.sh or pass --embed-model", candidates())
            })?;
            Embedder::load(&dir)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// Whether the model is loadable, without loading it (for /health and the webui).
pub fn status() -> serde_json::Value {
    match EMBEDDER.get() {
        Some(Ok(e)) => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": true, "dir": e.dir}),
        Some(Err(err)) => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": false, "error": err}),
        None => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": false, "dir": find_model_dir()}),
    }
}

impl Embedder {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let device = Device::Cpu;
        let cfg_text = std::fs::read_to_string(dir.join("config.json")).map_err(|e| format!("config.json: {e}"))?;
        let config: Config = serde_json::from_str(&cfg_text).map_err(|e| format!("config.json: {e}"))?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| format!("tokenizer.json: {e}"))?;
        tokenizer.with_padding(Some(PaddingParams::default()));
        tokenizer
            .with_truncation(Some(TruncationParams { max_length: MAX_TOKENS, ..Default::default() }))
            .map_err(|e| format!("tokenizer truncation: {e}"))?;
        // SAFETY: the file is only read; mmap is candle's standard loader
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DType::F32, &device) }
            .map_err(|e| format!("model.safetensors: {e}"))?;
        let model = BertModel::load(vb, &config).map_err(|e| format!("bert: {e}"))?;
        Ok(Self { model, tokenizer, device, dir: dir.to_path_buf() })
    }

    /// One normalised 384-dim vector per text (CPU-bound: call from spawn_blocking).
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            out.extend(self.embed_batch(batch).map_err(|e| format!("embed: {e}"))?);
        }
        Ok(out)
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        let enc = self.tokenizer.encode_batch(texts.to_vec(), true)?;
        let ids: Vec<Tensor> = enc.iter().map(|e| Tensor::new(e.get_ids(), &self.device)).collect::<Result<_, _>>()?;
        let mask: Vec<Tensor> = enc.iter().map(|e| Tensor::new(e.get_attention_mask(), &self.device)).collect::<Result<_, _>>()?;
        let ids = Tensor::stack(&ids, 0)?;
        let mask = Tensor::stack(&mask, 0)?;
        let types = ids.zeros_like()?;
        let hidden = self.model.forward(&ids, &types, Some(&mask))?; // [b, t, 384]
        // mean over real tokens only
        let m = mask.to_dtype(DType::F32)?.unsqueeze(2)?; // [b, t, 1]
        let summed = hidden.broadcast_mul(&m)?.sum(1)?; // [b, 384]
        let counts = m.sum(1)?.clamp(1e-9, f64::MAX)?; // [b, 1]
        let mean = summed.broadcast_div(&counts)?;
        let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?.clamp(1e-12, f64::MAX)?;
        let unit = mean.broadcast_div(&norm)?;
        Ok(unit.to_vec2::<f32>()?)
    }
}

/// Embed off the async runtime.
pub async fn embed_async(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    tokio::task::spawn_blocking(move || get()?.embed(&texts))
        .await
        .map_err(|e| format!("embed task: {e}"))?
}

#[cfg(test)]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the downloaded model (tools/fetch_model.sh); skipped without it.
    #[test]
    fn similar_sentences_are_closer_than_unrelated_ones() {
        let Some(dir) = find_model_dir() else {
            eprintln!("skipped: no {MODEL_NAME} directory");
            return;
        };
        let e = Embedder::load(&dir).unwrap();
        let v = e
            .embed(&[
                "The engine claims a work item and runs the agent.".into(),
                "An agent is started after the engine takes a queued task.".into(),
                "Bananas are rich in potassium.".into(),
            ])
            .unwrap();
        assert_eq!(v[0].len(), DIM);
        let n: f32 = v[0].iter().map(|x| x * x).sum();
        assert!((n - 1.0).abs() < 1e-3, "unit length, got {n}");
        assert!(cosine(&v[0], &v[1]) > cosine(&v[0], &v[2]) + 0.2);
    }
}
