//! Local embedding generation and cosine similarity for semantic search over the content cache.
//!
//! ## Approach
//!
//! Feature hashing (the "hashing trick"), not a neural embedding pipeline:
//! trades true semantic matching for zero network calls, no C-binding runtime, and no model download,
//! appropriate for a modest local dataset that does not need a production vector database.
//!
//! [`embed_text`] tokenizes via [`unicode_segmentation`] (locale-aware word boundaries),
//! hashes each lowercased token with [`xxhash_rust`]'s `xxh3` into one of [`EMBED_DIM`] buckets,
//! and accumulates a signed count per bucket (the sign bit also comes from the hash,
//! avoiding systematic bias from hash collisions).
//! The resulting vector is L2-normalized, so cosine similarity reduces to a plain dot product.
//! Same technique as scikit-learn's `HashingVectorizer` and Vowpal Wabbit.
//!
//! Limitation: lexical/topical similarity only, not true synonym matching (e.g. "car" vs "automobile").
//! An upgrade path exists: swap [`embed_text`] for a real local transformer embedding (`candle`),
//! without touching [`cosine_similarity`], the schema, or any query call site,
//! since every one of them only ever sees a fixed-size `Vec<f32>`.
//!
//! [`unicode_segmentation`]: https://docs.rs/unicode-segmentation
//! [`xxhash_rust`]: https://docs.rs/xxhash-rust

use std::mem::size_of;

use ndarray::Array1;
use unicode_segmentation::UnicodeSegmentation;
use xxhash_rust::xxh3::xxh3_64;

/// Fixed embedding dimensionality.
/// 128 keeps the per-row BLOB small (512 bytes as `f32`),
/// while giving the hashing trick enough buckets that unrelated tokens rarely collide,
/// for the vocabulary size a single mmcp mirror realistically carries.
pub const EMBED_DIM: usize = 128;

/// Bit position of the sign bit within a 64-bit `xxh3_64` hash.
/// Derived from `u64::BITS` rather than a bare `63` literal,
/// so the intent ("the top bit of the hash") stays self-evident.
const HASH_SIGN_BIT_SHIFT: u32 = u64::BITS - 1;

/// Embed `text` into an [`EMBED_DIM`]-dimensional, L2-normalized vector via feature hashing.
/// See the module docs for the rationale.
/// Empty or all-whitespace input yields the zero vector,
/// (cosine similarity against a zero vector is always `0.0`,
/// which is the correct "unrelated" answer for empty content).
#[must_use]
pub fn embed_text(text: &str) -> Vec<f32> {
    let mut buckets = [0f32; EMBED_DIM];
    for word in text.unicode_words() {
        let lower = word.to_lowercase();
        let hash = xxh3_64(lower.as_bytes());
        let bucket = (hash % EMBED_DIM as u64) as usize;
        // Re-use a different bit of the same hash for the sign,
        // so a single hash call decides both placement and polarity:
        // the standard hashing-trick construction,
        // to keep collisions from systematically biasing any one bucket upward.
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

/// Zero-allocation dot product over two equal-length slices.
/// The shared primitive [`cosine_similarity`] and
/// [`cosine_similarity_with_query_norm`] build on, so neither pays
/// for an [`Array1`] copy of its inputs just to multiply and sum.
#[must_use]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// L2 norm of `vector`, via [`dot`] rather than an [`Array1`] copy.
#[must_use]
pub fn norm(vector: &[f32]) -> f32 {
    dot(vector, vector).sqrt()
}

/// Cosine similarity between two embeddings of equal length.
/// Both [`embed_text`] outputs are already L2-normalized, so a plain dot product already equals cosine similarity;
/// this function still normalizes defensively,
/// so a caller holding a non-normalized vector (e.g. one decoded from an older schema),
/// gets a correct answer rather than a silently wrong one.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    cosine_similarity_with_query_norm(a, norm(a), b)
}

/// Same contract as [`cosine_similarity`], but takes `a`'s L2 norm
/// pre-computed by the caller.
///
/// A caller scoring one query against many rows hoists the query norm out of the loop.
/// `b`'s norm is recomputed per call, since `b` differs per row.
#[must_use]
pub fn cosine_similarity_with_query_norm(a: &[f32], a_norm: f32, b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let denom = a_norm * norm(b);
    if denom == 0.0 { 0.0 } else { dot(a, b) / denom }
}

/// Serialize an embedding vector to little-endian `f32` bytes for the `indexed_memory.embedding` BLOB column.
#[must_use]
pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Inverse of [`vector_to_bytes`].
/// Ignores a trailing partial `f32`.
/// A BLOB whose length is not a multiple of `size_of::<f32>()` can only come from external tampering.
/// `vector_to_bytes` always emits a length that is a multiple of `size_of::<f32>()`.
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
