//! Local embedding generation and cosine similarity for semantic
//! search over the content cache.
//!
//! ## Approach and why
//!
//! mmcp needs "search that tolerates a different choice of words",
//! entirely offline, with no network call, no C-binding runtime, and
//! no model download — and, per the operator's own scoping, over a
//! "modest local dataset" that explicitly does not need a production
//! vector database. A full neural sentence-embedding pipeline
//! (tokenizer + a transformer model run through `candle` or an ONNX
//! runtime) would satisfy "real semantic search" more thoroughly,
//! but drags in a model-weight download/cache step and a
//! meaningfully larger, slower-to-validate dependency surface for a
//! problem this module is explicitly scoped not to need at mmcp's
//! current scale.
//!
//! Instead this module uses **feature hashing** (the "hashing
//! trick"): tokenize text into Unicode words via
//! [`unicode_segmentation`] (pure Rust, locale-aware word
//! boundaries — not a hand-rolled whitespace split), hash each
//! lowercased token with [`xxhash_rust`]'s `xxh3` (pure Rust,
//! non-cryptographic, fast) into one of [`EMBED_DIM`] buckets, and
//! accumulate a signed count per bucket (the sign bit also comes
//! from the hash, the standard technique to keep hash collisions
//! from systematically biasing a bucket). The resulting vector is
//! L2-normalized so cosine similarity between two vectors reduces to
//! a plain dot product. This is the same technique behind
//! scikit-learn's `HashingVectorizer` and Vowpal Wabbit — a real,
//! established, well-understood approach, not an ad hoc invention —
//! and it captures lexical/topical overlap well for the short
//! English-heavy memory names, descriptions, and bodies this cache
//! indexes, without needing a trained model, a download, or any
//! runtime beyond pure Rust arithmetic.
//!
//! Honest limitation: this is a lexical/topical similarity measure,
//! not a deep semantic one — it will not match true synonyms with
//! zero shared vocabulary (e.g. "car" vs "automobile"). If that
//! proves insufficient in practice, the natural upgrade path is
//! swapping this module's [`embed_text`] for a real local
//! transformer embedding (`candle` is the pure-Rust-CPU-backend
//! candidate to reach for, per the project's pure-Rust preference)
//! without touching [`cosine_similarity`], the schema, or any query
//! call site, since every one of them only ever sees a fixed-size
//! `Vec<f32>`.
//!
//! [`unicode_segmentation`]: https://docs.rs/unicode-segmentation
//! [`xxhash_rust`]: https://docs.rs/xxhash-rust

use std::mem::size_of;

use ndarray::Array1;
use unicode_segmentation::UnicodeSegmentation;
use xxhash_rust::xxh3::xxh3_64;

/// Fixed embedding dimensionality. 128 keeps the per-row BLOB small
/// (512 bytes as `f32`) while giving the hashing trick enough
/// buckets that unrelated tokens rarely collide for the vocabulary
/// size a single mmcp mirror realistically carries.
pub const EMBED_DIM: usize = 128;

/// Bit position of the sign bit within a 64-bit `xxh3_64` hash.
/// Derived from `u64::BITS` rather than a bare `63` literal so the
/// intent ("the top bit of the hash") stays self-evident.
const HASH_SIGN_BIT_SHIFT: u32 = u64::BITS - 1;

/// Embed `text` into an [`EMBED_DIM`]-dimensional, L2-normalized
/// vector via feature hashing. See the module docs for the
/// rationale. Empty or all-whitespace input yields the zero vector
/// (cosine similarity against a zero vector is always `0.0`, which
/// is the correct "unrelated" answer for empty content).
#[must_use]
pub fn embed_text(text: &str) -> Vec<f32> {
    let mut buckets = [0f32; EMBED_DIM];
    for word in text.unicode_words() {
        let lower = word.to_lowercase();
        let hash = xxh3_64(lower.as_bytes());
        let bucket = (hash % EMBED_DIM as u64) as usize;
        // Re-use a different bit of the same hash for the sign so a
        // single hash call decides both placement and polarity; the
        // standard hashing-trick construction to keep collisions
        // from systematically biasing any one bucket upward.
        let sign = if (hash >> HASH_SIGN_BIT_SHIFT) & 1 == 1 {
            1.0
        } else {
            -1.0
        };
        buckets[bucket] += sign;
    }
    let vector = Array1::from_vec(buckets.to_vec());
    let norm = vector.dot(&vector).sqrt();
    if norm > 0.0 {
        (vector / norm).to_vec()
    } else {
        vector.to_vec()
    }
}

/// Cosine similarity between two embeddings of equal length via
/// [`ndarray`]'s dot product rather than a hand-rolled loop. Both
/// [`embed_text`] outputs are already L2-normalized, so a plain dot
/// product already equals cosine similarity; this function still
/// normalizes defensively so a caller holding a non-normalized
/// vector (e.g. one decoded from an older schema) gets a correct
/// answer rather than a silently wrong one.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let va = Array1::from_vec(a.to_vec());
    let vb = Array1::from_vec(b.to_vec());
    let denom = (va.dot(&va).sqrt()) * (vb.dot(&vb).sqrt());
    if denom == 0.0 {
        0.0
    } else {
        va.dot(&vb) / denom
    }
}

/// Serialize an embedding vector to little-endian `f32` bytes for
/// the `indexed_memory.embedding` BLOB column.
#[must_use]
pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Inverse of [`vector_to_bytes`]. Ignores a trailing partial `f32`
/// (a BLOB whose length is not a multiple of `size_of::<f32>()` can
/// only come from external tampering, never from `vector_to_bytes`).
#[must_use]
pub fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(size_of::<f32>())
        .map(|chunk| {
            f32::from_le_bytes(
                chunk
                    .try_into()
                    .expect("chunks_exact(size_of::<f32>()) yields exactly that many bytes"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_text_is_l2_normalized() {
        let v = embed_text("the quick brown fox jumps over the lazy dog");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5 || norm == 0.0,
            "expected a unit vector, got norm {norm}"
        );
    }

    #[test]
    fn empty_text_embeds_to_the_zero_vector() {
        let v = embed_text("   ");
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn identical_text_is_maximally_similar() {
        let v = embed_text("quokkas are small wallaby-like marsupials");
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn overlapping_vocabulary_scores_higher_than_disjoint_vocabulary() {
        let quokka = embed_text("the quokka is a small marsupial native to Australia");
        let quokka_again = embed_text("quokkas are marsupials found in Australia");
        let unrelated = embed_text("the stock market closed higher today on strong earnings");

        let related_score = cosine_similarity(&quokka, &quokka_again);
        let unrelated_score = cosine_similarity(&quokka, &unrelated);
        assert!(
            related_score > unrelated_score,
            "related={related_score} unrelated={unrelated_score}"
        );
    }

    #[test]
    fn byte_roundtrip_preserves_the_vector() {
        let v = embed_text("round trip test vector");
        let bytes = vector_to_bytes(&v);
        let back = bytes_to_vector(&bytes);
        assert_eq!(v, back);
    }
}
