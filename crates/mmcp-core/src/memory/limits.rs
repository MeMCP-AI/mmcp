//! Bounded-length validation for a memory's external string
//! fields.
//!
//! `global-security-rules` (mandatory mmcp memory) requires every
//! string field on an external input to carry an explicit maximum
//! length. Before this module existed, only the memory slug was
//! bounded (`MAX_SLUG_LENGTH` / `MAX_SLUG_SEGMENTS` in
//! `mmcp-store`); `body`, `name`, `description`, `tags`, and the
//! optional commit-message override taken by `write_memory` /
//! `edit_memory` / `import_memory` / `add_feature` were all
//! unbounded `String`s, so an unbounded body was buffered in
//! memory and then committed as a permanent, non-reclaimable git
//! blob.
//!
//! This module is the single owner of those bounds: the named
//! constants below are the SSOT every crate consults, and
//! [`validate_field_length`] / [`validate_tags`] /
//! [`validate_frontmatter_lengths`] / [`validate_body_length`] /
//! [`validate_message_length`] are the checks that enforce them.
//! Callers apply these at the write boundary, before the checked
//! value becomes a committed domain value. `mmcp_store::memory`
//! validates frontmatter and body at `write_file_at_path`, and
//! resolves every commit message (write, delete, move, and the
//! feature/issue renames) through its own
//! `resolve_commit_message`, so a caller-supplied message can
//! never reach git unbounded regardless of entry point.
//!
//! The maxima below are audited against real data already
//! committed to this project's own mmcp mirror (global group
//! `019d9567-748a-73c3-afc4-546e976ea1e0` and project group
//! `019d955d-4cce-77f2-a0b3-0b79ed394612`) rather than picked from
//! assumption: a bound must never reject data that legitimately
//! already exists in the mirror. See each constant's doc comment
//! for the observed maximum and the margin chosen above it.

use crate::memory::MemoryFrontmatter;

/// Maximum byte length of a memory's `name` (the human-readable
/// title shown in listings and the WebUI). The longest real name
/// across the global and mmcp project groups is 145 bytes
/// (`git-checkout-collision-forces-cherry-pick-fallback-when-worker-branches-stay-checked-out-2026-08-01`'s
/// title). 256 bytes keeps roughly 1.75x headroom above that
/// observed maximum while staying far below anything a UI would
/// reasonably render on a single line.
pub const MAX_NAME_LENGTH: usize = 256;

/// Maximum byte length of a memory's `description` (the one-line
/// summary the AI uses for relevance inference, per
/// [`MemoryFrontmatter`]'s own doc comment). The longest real
/// description observed across this project's own mirror is 1084
/// bytes
/// (`infra-weekly-api-limit-hit-2026-08-01-resets-2026-08-05`), with
/// multiple others above 600-900 bytes. 2048 bytes keeps roughly
/// 1.9x headroom above that observed maximum: a real bound meant to
/// stop pathological input, not the project's own descriptive
/// prose.
pub const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// Maximum byte length of a single tag. Tags are short
/// classification labels, not free text. The longest real tag
/// observed is 21 bytes (`improvement-candidate`), so 64 bytes
/// keeps roughly 3x headroom while bounding pathological input.
pub const MAX_TAG_LENGTH: usize = 64;

/// Maximum number of tags a single memory may carry. The most tags
/// any real memory carries is 8, so 32 keeps 4x headroom above that
/// observed maximum while bounding an unbounded-list attack on the
/// frontmatter.
pub const MAX_TAG_COUNT: usize = 32;

/// Maximum byte length of a memory body. Memories are Markdown
/// documents, not blob storage. The longest real bodies observed
/// (multi-section rule memories such as
/// `global-worktree-orchestration`) run to a few tens of KiB, so the
/// existing 512 KiB (`512 * 1024` bytes) bound is untouched by real
/// data and stays generous for even a long-form design document or a
/// large generated report, while keeping a single memory from
/// becoming a multi-gigabyte, permanent, non-reclaimable git blob.
pub const MAX_BODY_LENGTH: usize = 512 * 1024;

/// Maximum byte length of an explicit commit-message override
/// accepted by `write_memory` / `edit_memory` / `import_memory` /
/// `add_feature` / `move_memory` / `rename_feature` / `rename_issue`.
/// The longest real commit message observed is 1110 bytes
/// (subject + body of `resolve(fr-45): notes channel ships across
/// MCP + CLI`), which the mandatory multi-bullet commit-body
/// convention routinely produces. 4096 bytes keeps roughly 3.7x
/// headroom above that observed maximum; when the caller omits a
/// message the server synthesizes one and this check never runs.
pub const MAX_MESSAGE_LENGTH: usize = 4096;

/// A bounded external string or list field exceeded its maximum.
#[derive(Debug, thiserror::Error)]
pub enum FieldLengthError {
    /// A single string field (body, name, description, one tag, or
    /// the commit-message override) exceeded its byte-length
    /// maximum.
    #[error("field '{field}' is too long: {actual} bytes exceeds the {max}-byte maximum")]
    TooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },

    /// A list field (currently only `tags`) carried more entries
    /// than its maximum count.
    #[error("field '{field}' has too many entries: {actual} exceeds the {max}-entry maximum")]
    TooMany {
        field: &'static str,
        max: usize,
        actual: usize,
    },
}

/// Check a single string field's byte length against `max`,
/// tagging a failure with `field`'s name so callers (and the MCP
/// error payload built from it) can point at the exact offending
/// field.
pub fn validate_field_length(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), FieldLengthError> {
    let actual = value.len();
    if actual > max {
        return Err(FieldLengthError::TooLong { field, max, actual });
    }
    Ok(())
}

/// Validate a memory's tag list: bounded entry count via
/// [`MAX_TAG_COUNT`], and every individual tag bounded in length
/// via [`MAX_TAG_LENGTH`].
pub fn validate_tags(tags: &[String]) -> Result<(), FieldLengthError> {
    if tags.len() > MAX_TAG_COUNT {
        return Err(FieldLengthError::TooMany {
            field: "tags",
            max: MAX_TAG_COUNT,
            actual: tags.len(),
        });
    }
    for tag in tags {
        validate_field_length("tag", tag, MAX_TAG_LENGTH)?;
    }
    Ok(())
}

/// Validate every bounded field on a memory's frontmatter: `name`,
/// `description`, and `tags`. Body length is validated separately
/// via [`validate_body_length`] since the body lives outside
/// [`MemoryFrontmatter`].
pub fn validate_frontmatter_lengths(
    frontmatter: &MemoryFrontmatter,
) -> Result<(), FieldLengthError> {
    validate_field_length("name", &frontmatter.name, MAX_NAME_LENGTH)?;
    validate_field_length(
        "description",
        &frontmatter.description,
        MAX_DESCRIPTION_LENGTH,
    )?;
    validate_tags(&frontmatter.tags)
}

/// Validate a memory body against [`MAX_BODY_LENGTH`].
pub fn validate_body_length(body: &str) -> Result<(), FieldLengthError> {
    validate_field_length("body", body, MAX_BODY_LENGTH)
}

/// Validate an explicit commit-message override against
/// [`MAX_MESSAGE_LENGTH`]. Callers only invoke this when the
/// caller actually supplied a message; the server-synthesized
/// default used when no override is given never needs checking.
pub fn validate_message_length(message: &str) -> Result<(), FieldLengthError> {
    validate_field_length("message", message, MAX_MESSAGE_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryKind;

    #[test]
    fn field_length_accepts_at_limit() {
        let value = "a".repeat(MAX_NAME_LENGTH);
        assert!(validate_field_length("name", &value, MAX_NAME_LENGTH).is_ok());
    }

    #[test]
    fn field_length_rejects_over_limit() {
        let value = "a".repeat(MAX_NAME_LENGTH + 1);
        let err = validate_field_length("name", &value, MAX_NAME_LENGTH).unwrap_err();
        match err {
            FieldLengthError::TooLong { field, max, actual } => {
                assert_eq!(field, "name");
                assert_eq!(max, MAX_NAME_LENGTH);
                assert_eq!(actual, MAX_NAME_LENGTH + 1);
            }
            FieldLengthError::TooMany { .. } => panic!("expected TooLong"),
        }
    }

    #[test]
    fn name_at_and_over_limit() {
        assert!(
            validate_field_length("name", &"a".repeat(MAX_NAME_LENGTH), MAX_NAME_LENGTH).is_ok()
        );
        assert!(
            validate_field_length("name", &"a".repeat(MAX_NAME_LENGTH + 1), MAX_NAME_LENGTH)
                .is_err()
        );
    }

    #[test]
    fn description_at_and_over_limit() {
        assert!(
            validate_field_length(
                "description",
                &"a".repeat(MAX_DESCRIPTION_LENGTH),
                MAX_DESCRIPTION_LENGTH
            )
            .is_ok()
        );
        assert!(
            validate_field_length(
                "description",
                &"a".repeat(MAX_DESCRIPTION_LENGTH + 1),
                MAX_DESCRIPTION_LENGTH
            )
            .is_err()
        );
    }

    #[test]
    fn body_at_and_over_limit() {
        assert!(validate_body_length(&"a".repeat(MAX_BODY_LENGTH)).is_ok());
        assert!(validate_body_length(&"a".repeat(MAX_BODY_LENGTH + 1)).is_err());
    }

    #[test]
    fn message_at_and_over_limit() {
        assert!(validate_message_length(&"a".repeat(MAX_MESSAGE_LENGTH)).is_ok());
        assert!(validate_message_length(&"a".repeat(MAX_MESSAGE_LENGTH + 1)).is_err());
    }

    #[test]
    fn tag_length_at_and_over_limit() {
        assert!(validate_tags(&[("a".repeat(MAX_TAG_LENGTH))]).is_ok());
        assert!(validate_tags(&[("a".repeat(MAX_TAG_LENGTH + 1))]).is_err());
    }

    #[test]
    fn tag_count_at_and_over_limit() {
        let at_limit: Vec<String> = (0..MAX_TAG_COUNT).map(|i| format!("t{i}")).collect();
        assert!(validate_tags(&at_limit).is_ok());
        let over_limit: Vec<String> = (0..=MAX_TAG_COUNT).map(|i| format!("t{i}")).collect();
        match validate_tags(&over_limit).unwrap_err() {
            FieldLengthError::TooMany { field, max, actual } => {
                assert_eq!(field, "tags");
                assert_eq!(max, MAX_TAG_COUNT);
                assert_eq!(actual, MAX_TAG_COUNT + 1);
            }
            FieldLengthError::TooLong { .. } => panic!("expected TooMany"),
        }
    }

    #[test]
    fn frontmatter_lengths_accept_valid_and_reject_oversized() {
        let ok = MemoryFrontmatter::new("a".repeat(MAX_NAME_LENGTH), "d", MemoryKind::Rule);
        assert!(validate_frontmatter_lengths(&ok).is_ok());

        let bad_name =
            MemoryFrontmatter::new("a".repeat(MAX_NAME_LENGTH + 1), "d", MemoryKind::Rule);
        assert!(validate_frontmatter_lengths(&bad_name).is_err());

        let bad_description = MemoryFrontmatter::new(
            "n",
            "a".repeat(MAX_DESCRIPTION_LENGTH + 1),
            MemoryKind::Rule,
        );
        assert!(validate_frontmatter_lengths(&bad_description).is_err());

        let bad_tags = MemoryFrontmatter::new("n", "d", MemoryKind::Rule)
            .with_tags(vec!["a".repeat(MAX_TAG_LENGTH + 1)]);
        assert!(validate_frontmatter_lengths(&bad_tags).is_err());
    }
}
