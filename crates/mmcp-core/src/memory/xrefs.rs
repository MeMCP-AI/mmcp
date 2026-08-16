//! Shared parsers for typed cross-reference arguments.
//!
//! `parse_cross_refs` and `parse_memory_refs` are the kind-agnostic
//! validators that every tracker module (`features`, `issues`, …)
//! and every external surface (CLI flag handlers, MCP tool methods)
//! funnel raw `depends_on` / `blocks` / `refs` input through. Living
//! in `mmcp-core::memory` keeps them one extraction away from the
//! `MemoryRef` types they validate, and stops consumer crates from
//! reaching across at peer modules.
//!
//! The parsers belong in their own concern-named module rather than
//! `pub use`d from one consumer to another, so feature and issue
//! surfaces share one validator without coupling.

use uuid::Uuid;

use crate::memory::MemoryRef;

/// Wire-form `(target, commit)` pair shared by every entry point
/// that accepts typed memory references.
///
/// Plain strings on the wire so MCP, CLI, and any other surface
/// can pass raw user input through [`parse_memory_refs`] without
/// pre-validating UUIDs or commit shapes themselves. Both fields
/// are mandatory: there is no shape of this type that omits the
/// commit pin (per the supersede-convention rule that every ref
/// carries a sha).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRefInput {
    /// UUID string of the referenced memory. Validated by
    /// [`parse_memory_refs`].
    pub target: String,
    /// 40-char lowercase hex commit sha pinning the reference.
    /// Validated by [`MemoryRef::validate_commit_shape`].
    pub commit: String,
}

/// Errors raised by [`parse_cross_refs`] and [`parse_memory_refs`].
///
/// Each variant names the field it came from (so a single tool
/// surface can route `depends_on` errors and `refs` errors to the
/// same handler without re-parsing the message). Variants are
/// `Clone + PartialEq` so consumer test suites can match on them
/// without cloning through `Box<dyn Error>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum XrefError {
    /// A `depends_on` / `blocks` entry could not be parsed as a
    /// UUID. `field` is the static field name (`"depends_on"` /
    /// `"blocks"`); `value` is the offending input string.
    #[error("cross-reference '{value}' on field `{field}` is not a valid UUID")]
    InvalidCrossRef {
        /// Static field name attached by the caller
        /// (e.g. `"depends_on"`).
        field: &'static str,
        /// Offending input echoed back so the message is
        /// self-describing.
        value: String,
    },
    /// A typed `refs` entry was malformed: either the `target`
    /// did not parse as a UUID or the `commit` failed the
    /// 40-char lowercase hex check.
    #[error("memory reference on field `{field}`: {detail}")]
    InvalidMemoryRef {
        /// Static field name attached by the caller
        /// (e.g. `"refs"`, `"superseded_by"`).
        field: &'static str,
        /// Human-readable diagnosis of the specific shape error.
        detail: String,
    },
}

/// Parse a list of raw cross-reference strings into the
/// `Vec<Uuid>` shape that tracker metadata blocks expect.
///
/// `field` is the caller-supplied static name attached to every
/// error (so a single error handler can branch by field without
/// re-parsing the message). Returns the parsed UUIDs in the same
/// order as the input on success.
pub fn parse_cross_refs(values: &[String], field: &'static str) -> Result<Vec<Uuid>, XrefError> {
    values
        .iter()
        .map(|raw| {
            Uuid::parse_str(raw).map_err(|_| XrefError::InvalidCrossRef {
                field,
                value: raw.clone(),
            })
        })
        .collect()
}

/// Parse a list of raw [`MemoryRefInput`] pairs into a list of
/// validated [`MemoryRef`]s.
///
/// Validates UUID shape on `target` and commit shape on `commit`;
/// rejects either malformed half through [`XrefError::InvalidMemoryRef`].
pub fn parse_memory_refs(
    values: &[MemoryRefInput],
    field: &'static str,
) -> Result<Vec<MemoryRef>, XrefError> {
    values
        .iter()
        .map(|raw| {
            let target = Uuid::parse_str(&raw.target).map_err(|_| XrefError::InvalidMemoryRef {
                field,
                detail: format!("target '{}' is not a valid UUID", raw.target),
            })?;
            MemoryRef::validate_commit_shape(&raw.commit).map_err(|e| {
                XrefError::InvalidMemoryRef {
                    field,
                    detail: format!("{e}"),
                }
            })?;
            Ok(MemoryRef::new(target, raw.commit.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn forty_char_hex() -> &'static str {
        "0123456789abcdef0123456789abcdef01234567"
    }

    #[test]
    fn parse_cross_refs_accepts_valid_uuids_in_order() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let parsed =
            parse_cross_refs(&[a.to_string(), b.to_string()], "depends_on").expect("parse ok");
        assert_eq!(parsed, vec![a, b]);
    }

    #[test]
    fn parse_cross_refs_rejects_garbage_with_field_attribution() {
        let err = parse_cross_refs(&["not-a-uuid".to_string()], "blocks")
            .expect_err("invalid uuid must error");
        assert_eq!(
            err,
            XrefError::InvalidCrossRef {
                field: "blocks",
                value: "not-a-uuid".to_string(),
            }
        );
    }

    #[test]
    fn parse_cross_refs_empty_input_returns_empty_output() {
        let parsed = parse_cross_refs(&[], "depends_on").expect("empty parse ok");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_memory_refs_accepts_valid_pair() {
        let target = Uuid::now_v7();
        let commit = forty_char_hex().to_string();
        let parsed = parse_memory_refs(
            &[MemoryRefInput {
                target: target.to_string(),
                commit: commit.clone(),
            }],
            "refs",
        )
        .expect("parse ok");
        assert_eq!(parsed, vec![MemoryRef::new(target, &commit)]);
    }

    #[test]
    fn parse_memory_refs_rejects_invalid_uuid_with_field_attribution() {
        let err = parse_memory_refs(
            &[MemoryRefInput {
                target: "not-a-uuid".to_string(),
                commit: forty_char_hex().to_string(),
            }],
            "refs",
        )
        .expect_err("invalid uuid must error");
        match err {
            XrefError::InvalidMemoryRef { field, detail } => {
                assert_eq!(field, "refs");
                assert!(
                    detail.contains("not-a-uuid"),
                    "detail must echo input: {detail}"
                );
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn parse_memory_refs_rejects_short_commit_with_field_attribution() {
        let target = Uuid::now_v7();
        let err = parse_memory_refs(
            &[MemoryRefInput {
                target: target.to_string(),
                commit: "abc".to_string(),
            }],
            "superseded_by",
        )
        .expect_err("short commit must error");
        match err {
            XrefError::InvalidMemoryRef { field, .. } => {
                assert_eq!(field, "superseded_by");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn parse_memory_refs_empty_input_returns_empty_output() {
        let parsed = parse_memory_refs(&[], "refs").expect("empty parse ok");
        assert!(parsed.is_empty());
    }
}
