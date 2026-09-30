// Embeds the iter4 user guide (docs/iter4_guide.md) when it exists, so the
// build never depends on it; GraphRAG ingests it as a product-wide document
// (iter_data/src/rag/guide.rs).
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let guide = std::path::Path::new(&manifest).join("../docs/iter4_guide.md");
    println!("cargo:rerun-if-changed={}", guide.display());
    println!("cargo:rerun-if-changed={}", guide.parent().unwrap().display());
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("guide.rs");
    let body = if guide.is_file() {
        format!("pub const GUIDE: &str = include_str!({:?});\n", guide.canonicalize().unwrap())
    } else {
        "pub const GUIDE: &str = \"\";\n".to_string()
    };
    std::fs::write(out, body).unwrap();
}
