//! Typed cross-reference from one memory (or feature) to another.
//!
//! A [`MemoryRef`] is a (target uuid, commit sha) pair. The commit
//! pin is mandatory: every reference anywhere in mmcp metadata
//! carries the git revision at which the referenced memory was in
//! the state the referrer cared about, so renames, rewrites, or
//! later supersession on the target side never silently change
//! what a reader sees when they follow the link.
//!
//! The type is deliberately narrow for v1 - just `target` and
//! `commit`. Richer shapes (a `kind` tag distinguishing memory /
//! feature / commit / log, or a `note` slot for human hints) are
//! tracked by FR-40 as an extension; the two-field form here is
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
    /// UUID of the referenced memory (or feature, since features
    /// are memories of kind `fr`). FR-028 makes this the primary
    /// key so refs survive slug renames.
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

#[cfg(test)]
mod tests {
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
    fn round_trips_through_serde_json_as_object() {
        // MCP wire format is JSON; the shape is a plain object.
        let id = Uuid::now_v7();
        let r = MemoryRef::new(id, forty_char_hex());
        let rendered = serde_json::to_string(&r).expect("render");
        let parsed: MemoryRef = serde_json::from_str(&rendered).expect("parse");
        assert_eq!(parsed, r);
    }
}
