// Copyright Alexandre D. Díaz
//! Shared text-embedding helper: the collector generates the vectors, and
//! both the mcp endpoint and the web server embed queries + cosine-rank
//! against them. Its own crate (not `oghutils`) to keep the ONNX runtime
//! dependency in one place.
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Folded into [`text_hash`] so switching models automatically invalidates
/// every stored vector (vectors from different models aren't comparable).
const MODEL_TAG: &str = "paraphrase-ml-minilm-l12-v2-q";

// ponytail: one hardcoded multilingual model behind a Mutex (fastembed's
// embed() needs &mut). Inference is serialized; fine for one collector run
// and low-QPS MCP queries.
static EMBEDDER: Mutex<Option<TextEmbedding>> = Mutex::new(None);

type Error = Box<dyn std::error::Error + Send + Sync>;

fn cache_dir() -> String {
    // Under data/ so the Docker volume keeps the downloaded model.
    std::env::var("OGHCOLLECTOR_EMBED_CACHE_DIR")
        .unwrap_or_else(|_| "data/.fastembed_cache".to_string())
}

/// Embeds a batch of texts (first call downloads/loads the model).
pub fn embed_texts<S: AsRef<str> + Send + Sync>(texts: &[S]) -> Result<Vec<Vec<f32>>, Error> {
    let mut guard = EMBEDDER.lock().unwrap();
    if guard.is_none() {
        *guard = Some(TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::ParaphraseMLMiniLML12V2Q)
                .with_cache_dir(cache_dir().into()),
        )?);
    }
    Ok(guard.as_mut().unwrap().embed(texts, Some(64))?)
}

/// Preloads the model in the background so the first real query doesn't pay
/// the download/startup cost. Failures only log - the caller can still work
/// (semantic search will just error per-request).
pub fn warmup() {
    if let Err(err) = embed_texts(&["warmup"]) {
        log::warn!("embedding model warmup failed: {err}");
    }
}

/// Fingerprint of the text (+ model tag) an embedding was generated from.
// ponytail: std DefaultHasher isn't guaranteed stable across Rust releases -
// worst case a toolchain bump re-embeds everything once. Swap for a fixed
// hash (e.g. sha2) if that ever hurts.
pub fn text_hash(text: &str) -> String {
    let mut h = DefaultHasher::new();
    MODEL_TAG.hash(&mut h);
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// f32 slice -> little-endian bytes, the module_embedding BLOB format.
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_to_vec(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Hybrid-ranks `items` (id, LE f32 BLOB as stored in module_embedding)
/// against an already-embedded query and returns the top `k` (id, score),
/// best first. `lexical` (id -> 0..1, e.g. sqlitedb's IDF-weighted
/// keyword-hit share) rescues rare proper nouns the embedding model can't
/// know ("Veri*Factu"): a full verbatim hit dominates the score while cosine
/// still breaks ties, and ids absent from the map keep their pure cosine.
/// Shared by the mcp tool and the server's /v1/semantic-search.
pub fn top_k<'a>(
    query_vec: &[f32],
    items: impl IntoIterator<Item = (i64, &'a [u8])>,
    lexical: &HashMap<i64, f32>,
    k: usize,
) -> Vec<(i64, f32)> {
    let mut scored: Vec<(i64, f32)> = items
        .into_iter()
        .map(|(id, bytes)| {
            let cos = cosine(query_vec, &bytes_to_vec(bytes));
            let lex = lexical.get(&id).copied().unwrap_or(0.0);
            (id, cos + (1.0 - cos) * lex * 0.9)
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(k);
    scored
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_roundtrip_and_cosine() {
        let v = vec![0.25f32, -1.5, 3.0];
        assert_eq!(bytes_to_vec(&vec_to_bytes(&v)), v);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_ne!(text_hash("a"), text_hash("b"));
    }

    #[test]
    fn top_k_ranks_and_truncates() {
        let close = vec_to_bytes(&[1.0, 0.0]);
        let far = vec_to_bytes(&[0.0, 1.0]);
        let mid = vec_to_bytes(&[1.0, 1.0]);
        let items = [
            (1i64, far.as_slice()),
            (2, close.as_slice()),
            (3, mid.as_slice()),
        ];
        let ranked = top_k(&[1.0, 0.0], items, &HashMap::new(), 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, 2);
        assert_eq!(ranked[1].0, 3);
    }

    #[test]
    fn top_k_lexical_hit_beats_plausible_cosine() {
        // cos ~0.707 - a realistically strong pure-semantic competitor.
        let mid = vec_to_bytes(&[1.0, 1.0]);
        let far = vec_to_bytes(&[0.0, 1.0]);
        let items = [(1i64, mid.as_slice()), (2, far.as_slice())];
        // Module 2 is semantically distant but matches the query keyword
        // verbatim (the Veri*Factu case) - it must outrank the cosine winner.
        let lexical = HashMap::from([(2i64, 1.0f32)]);
        let ranked = top_k(&[1.0, 0.0], items, &lexical, 2);
        assert_eq!(ranked[0].0, 2);
        assert!(ranked[0].1 > 0.85);
        // And an id absent from the map keeps its pure cosine.
        assert!((ranked[1].1 - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }
}
