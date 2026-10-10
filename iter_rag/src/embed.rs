//! The embedding model (GraphRAG, 2026-09-29): sentence-transformers
//! all-MiniLM-L6-v2 run in-process with candle (pure Rust, CPU) — 6 BERT
//! layers, 384-dim output, mean-pooled over the attention mask and
//! L2-normalised, so cosine similarity is a dot product.
//!
//! Who runs it (decided 2026-09-29): the **engine** embeds in bulk (chunks
//! and summaries — native, on the machine already doing the AI work);
//! **iter_data** embeds only search questions, so search never waits on an
//! engine. Every vector carries the model's **stamp** (name + the first 12
//! hex of the weights' sha256): a question embedded by different weights
//! than the chunks would score garbage, and the stamp makes that visible.
//!
//! The model directory is found once, lazily: `set_model_dir` (a flag) /
//! `$ITER_EMBED_MODEL`, else `models/all-MiniLM-L6-v2` beside the working
//! directory or the binary, else `/opt/iter/models/…` (the container), else
//! the per-user cache `~/.cache/iter/models/…`. `ensure_model` downloads it
//! into that cache when none is found (an engine machine needs no setup).
//!
//! Input past MAX_TOKENS word-pieces is cut off by the model itself (it was
//! trained on 256), so chunk.rs sizes chunks by counted tokens.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const MODEL_NAME: &str = "all-MiniLM-L6-v2";
pub const MODEL_REPO: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";
/// the weights this build was verified with (tools/fetch_model.sh checks the same)
pub const MODEL_SHA256: &str = "53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db";
pub const MODEL_FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors", "special_tokens_map.json", "tokenizer_config.json"];
pub const DIM: usize = 384;
pub const MAX_TOKENS: usize = 256;
/// texts per forward pass
const BATCH: usize = 16;

pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    /// the same vocabulary without padding or truncation, for counting
    counter: Tokenizer,
    device: Device,
    pub dir: PathBuf,
    /// "all-MiniLM-L6-v2@53aa51172d14"
    pub stamp: String,
}

static MODEL_DIR_FLAG: OnceLock<String> = OnceLock::new();
static EMBEDDER: OnceLock<Result<Embedder, String>> = OnceLock::new();

/// A `--embed-model` flag (before the first use).
pub fn set_model_dir(dir: &str) {
    if !dir.is_empty() {
        let _ = MODEL_DIR_FLAG.set(dir.to_string());
    }
}

fn cache_dir() -> Option<PathBuf> {
    // USERPROFILE: Windows sets no HOME (iter_rag does not depend on iter_core)
    let home = ["HOME", "USERPROFILE"].iter().filter_map(|k| std::env::var(k).ok()).find(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".cache/iter/models").join(MODEL_NAME))
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
        // target/release/iter_data or bin/iter_engine → iter4/models/…
        for anc in exe.ancestors().skip(1).take(4) {
            v.push(anc.join(&rel));
        }
    }
    v.push(PathBuf::from("/opt/iter/models").join(MODEL_NAME));
    if let Some(c) = cache_dir() {
        v.push(c);
    }
    v
}

fn complete(d: &Path) -> bool {
    d.join("model.safetensors").is_file() && d.join("tokenizer.json").is_file() && d.join("config.json").is_file()
}

pub fn find_model_dir() -> Option<PathBuf> {
    candidates().into_iter().find(|d| complete(d))
}

pub fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The model found on disk, else downloaded into the per-user cache
/// (checksum-verified). Blocking; an engine calls it once.
pub fn ensure_model() -> Result<PathBuf, String> {
    if let Some(d) = find_model_dir() {
        return Ok(d);
    }
    let dir = cache_dir().ok_or("no $HOME for the model cache")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let http = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(600)).build().map_err(|e| e.to_string())?;
    for f in MODEL_FILES {
        let url = format!("{MODEL_REPO}/{f}");
        let bytes = http.get(&url).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()).map_err(|e| format!("download {url}: {e}"))?;
        let part = dir.join(format!("{f}.part"));
        std::fs::write(&part, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&part, dir.join(f)).map_err(|e| e.to_string())?;
    }
    let got = sha256_file(&dir.join("model.safetensors"))?;
    if got != MODEL_SHA256 {
        return Err(format!("downloaded {MODEL_NAME} weights have sha256 {got}, expected {MODEL_SHA256}"));
    }
    Ok(dir)
}

/// The process-wide model, loaded on first use (downloading it if `download`).
fn load_global(download: bool) -> Result<&'static Embedder, String> {
    EMBEDDER
        .get_or_init(|| {
            let dir = if download { ensure_model()? } else {
                find_model_dir().ok_or_else(|| {
                    format!("embedding model {MODEL_NAME} not found (looked in {:?}); run iter4/tools/fetch_model.sh or pass --embed-model", candidates())
                })?
            };
            Embedder::load(&dir)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// The model from disk (iter_data: the container ships it).
pub fn get() -> Result<&'static Embedder, String> {
    load_global(false)
}

/// The model from disk or downloaded into the cache (engines).
pub fn get_or_download() -> Result<&'static Embedder, String> {
    load_global(true)
}

/// The stamp of the weights on disk, without loading them (hashes the file once).
pub fn disk_stamp() -> Option<String> {
    static STAMP: OnceLock<Option<String>> = OnceLock::new();
    STAMP
        .get_or_init(|| {
            let d = find_model_dir()?;
            sha256_file(&d.join("model.safetensors")).ok().map(|h| stamp_of(&h))
        })
        .clone()
}

pub fn stamp_of(sha: &str) -> String {
    format!("{MODEL_NAME}@{}", &sha[..12.min(sha.len())])
}

/// Whether the model is loadable, without loading it (for status pages).
pub fn status() -> serde_json::Value {
    match EMBEDDER.get() {
        Some(Ok(e)) => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": true, "dir": e.dir, "stamp": e.stamp}),
        Some(Err(err)) => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": false, "error": err}),
        None => serde_json::json!({"model": MODEL_NAME, "dim": DIM, "loaded": false, "dir": find_model_dir(), "stamp": disk_stamp()}),
    }
}

impl Embedder {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let device = Device::Cpu;
        let cfg_text = std::fs::read_to_string(dir.join("config.json")).map_err(|e| format!("config.json: {e}"))?;
        let config: Config = serde_json::from_str(&cfg_text).map_err(|e| format!("config.json: {e}"))?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| format!("tokenizer.json: {e}"))?;
        let mut counter = tokenizer.clone();
        counter.with_padding(None);
        counter.with_truncation(None).map_err(|e| format!("tokenizer: {e}"))?;
        tokenizer.with_padding(Some(PaddingParams::default()));
        tokenizer
            .with_truncation(Some(TruncationParams { max_length: MAX_TOKENS, ..Default::default() }))
            .map_err(|e| format!("tokenizer truncation: {e}"))?;
        let weights = dir.join("model.safetensors");
        let stamp = stamp_of(&sha256_file(&weights)?);
        // SAFETY: the file is only read; mmap is candle's standard loader
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights], DType::F32, &device) }.map_err(|e| format!("model.safetensors: {e}"))?;
        let model = BertModel::load(vb, &config).map_err(|e| format!("bert: {e}"))?;
        Ok(Self { model, tokenizer, counter, device, dir: dir.to_path_buf(), stamp })
    }

    /// Word-pieces the model would see for `text` (without [CLS]/[SEP]; no truncation).
    pub fn count_tokens(&self, text: &str) -> usize {
        self.counter.encode(text, false).map(|e| e.len()).unwrap_or(text.len() / 4)
    }

    /// One normalised 384-dim vector per text (CPU-bound).
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
        assert_eq!(e.stamp, stamp_of(MODEL_SHA256));
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
        // paths and identifiers cost many word-pieces
        assert!(e.count_tokens("{topdir}/iter_data/src/rag/mod.rs") > 12);
        assert!(e.count_tokens("the engine runs the agent") <= 6);
    }
}
