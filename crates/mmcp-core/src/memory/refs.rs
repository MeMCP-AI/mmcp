//! Typed cross-reference from one memory (or feature) to another.
//!
//! A [`MemoryRef`] is a (target uuid, commit sha) pair. The commit
//! pin is mandatory: every reference anywhere in mmcp metadata
//! carries the git revision at which the referenced memory was in
//! the state the referrer cared about, so renames, rewrites, or
//! later supersession on the target side never silently change
//! what a reader sees when they follow the link.
//!
//! The type is deliberately narrow: just `target` and
//! `commit`. Richer shapes (a `kind` tag distinguishing memory /
//! feature / commit / log, or a `note` slot for human hints) remain
//! a possible extension; the two-field form here is
//! forward compatible because serde will parse future optional
//! fields into the same struct once they are added.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A typed cross-reference to another memory at a specific git
/// commit.
///
/// Used on general [`MemoryFrontmatter::refs`](crate::memory::MemoryFrontmatter)
/// and on feature-specific
/// [`FeatureMetadata::superseded_by`](crate::memory::FeatureMetadata).
/// Bare UUID references are intentionally not supported: the whole
/// point of carrying a commit is to survive later edits on the
/// target side.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryRef {
    /// UUID of the referenced memory (or feature, since features are memories of kind `fr`).
    /// This UUID is the primary key, so refs survive slug renames.
    pub target: Uuid,

    /// 40-character lowercase hex commit sha pinning the target to
    /// a specific revision. Readers that want to follow the
    /// reference should resolve to this exact commit on the target
    /// group's repo; if the commit no longer exists (history was
    /// rewritten), the reference is stale and should surface a
    /// diagnostic.
    pub commit: String,
}

impl MemoryRef {
    /// Construct a new `MemoryRef`. Does not validate the commit
    /// shape - use [`MemoryRef::validate_commit_shape`] when the
    /// caller needs to reject malformed input at a boundary.
    #[must_use]
    pub fn new(target: Uuid, commit: impl Into<String>) -> Self {
        Self {
            target,
            commit: commit.into(),
        }
    }

    /// Return `Ok(())` when `commit` is a 40-character lowercase
    /// hex string, otherwise return the offending input as an
    /// [`InvalidCommit`] error.
    ///
    /// This is the same shape check a git plumbing tool uses when
    /// accepting a "full" commit id - abbreviated shas are
    /// intentionally rejected at the boundary so references are
    /// stable across repo history views that might disambiguate
    /// short shas differently.
    pub fn validate_commit_shape(commit: &str) -> Result<(), InvalidCommit> {
        if commit.len() != 40
            || !commit
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(InvalidCommit {
                input: commit.to_string(),
            });
        }
        Ok(())
    }
}

/// Raised when [`MemoryRef::validate_commit_shape`] sees a commit
/// string that does not match the 40-char lowercase hex contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid commit sha '{input}': expected 40 lowercase hex characters")]
pub struct InvalidCommit {
    /// The offending input echoed back for user-facing error
    /// messages.
    pub input: String,
}

/// Number of hex characters in a full (non-abbreviated) git commit
/// sha.
pub const COMMIT_SHA_HEX_LEN: usize = 40;

/// Whether `s` is SHAPED like a full git commit sha: exactly
/// [`COMMIT_SHA_HEX_LEN`] ASCII hex digits, either case.
///
/// The shared disambiguator every caller-supplied "revision" string
/// needs before choosing between `Rev::Commit` and `Rev::Branch` (a
/// bare slug/branch name never happens to be 40 hex characters in
/// practice). Deliberately laxer than
/// [`MemoryRef::validate_commit_shape`] (which also rejects
/// uppercase): a stored, canonical [`MemoryRef::commit`] must be
/// exact, but a caller picking a `Rev` variant only needs a shape
/// test, not a validated reference.
#[must_use]
pub fn looks_like_commit_sha(s: &str) -> bool {
    s.len() == COMMIT_SHA_HEX_LEN && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn forty_char_hex() -> &'static str {
        "0123456789abcdef0123456789abcdef01234567"
    }

    #[test]
    fn new_stores_target_and_commit_verbatim() {
        let id = Uuid::now_v7();
        let r = MemoryRef::new(id, forty_char_hex());
        assert_eq!(r.target, id);
        assert_eq!(r.commit, forty_char_hex());
    }

    #[test]
    fn validate_accepts_forty_char_lowercase_hex() {
        assert!(MemoryRef::validate_commit_shape(forty_char_hex()).is_ok());
    }

    #[test]
    fn validate_rejects_uppercase_hex() {
        let err = MemoryRef::validate_commit_shape("ABCDEF0123456789ABCDEF0123456789ABCDEF01")
            .expect_err("uppercase must fail");
        assert_eq!(err.input, "ABCDEF0123456789ABCDEF0123456789ABCDEF01");
    }

    #[test]
    fn validate_rejects_abbreviated_sha() {
        assert!(MemoryRef::validate_commit_shape("abc1234").is_err());
    }

    #[test]
    fn validate_rejects_non_hex_characters() {
        assert!(
            MemoryRef::validate_commit_shape("g123456789abcdef0123456789abcdef0123456z").is_err()
        );
    }

    #[test]
    fn validate_rejects_empty_string() {
        assert!(MemoryRef::validate_commit_shape("").is_err());
    }

    #[test]
    fn round_trips_through_toml_array_of_tables() {
        // The on-disk encoding for a `refs` list uses TOML
        // array-of-tables, one `[[refs]]` block per entry. Pin
        // that shape with a round-trip so accidental serde
        // renames break loudly.
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            refs: Vec<MemoryRef>,
        }
        let id = Uuid::now_v7();
        let original = Wrapper {
            refs: vec![MemoryRef::new(id, forty_char_hex())],
        };
        let rendered = toml::to_string(&original).expect("render");
        assert!(rendered.contains("[[refs]]"), "rendered: {rendered}");
        assert!(rendered.contains("target = "), "rendered: {rendered}");
        assert!(rendered.contains("commit = "), "rendered: {rendered}");
        let parsed: Wrapper = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn looks_like_commit_sha_accepts_forty_char_hex_either_case() {
        assert!(looks_like_commit_sha(forty_char_hex()));
        assert!(looks_like_commit_sha(
            "0123456789ABCDEF0123456789ABCDEF01234567"
        ));
    }

    #[test]
    fn looks_like_commit_sha_rejects_a_branch_name() {
        assert!(!looks_like_commit_sha("main"));
        assert!(!looks_like_commit_sha("feature/cross-group-milestone"));
    }

    #[test]
    fn looks_like_commit_sha_rejects_wrong_length_hex() {
        assert!(!looks_like_commit_sha("abc1234"));
        assert!(!looks_like_commit_sha(""));
    }

    #[test]
    fn round_trips_through_serde_json_as_object() {
        // MCP wire format is JSON; the shape is a plain object.
        let id = Uuid::now_v7();
        let r = MemoryRef::new(id, forty_char_hex());
        let rendered = serde_json::to_string(&r).expect("render");
        let parsed: MemoryRef = serde_json::from_str(&rendered).expect("parse");
        assert_eq!(parsed, r);
    }
}
