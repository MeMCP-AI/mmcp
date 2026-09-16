//! Typed pointer to a memory's pre-move history after a cross-group move.
//!
//! A cross-group move keeps the memory's id and body but starts a
//! fresh git history in the target group: commits do not travel
//! between repositories (see [`super::MemoryFrontmatter::history_source`]).
//! This pointer lets a reader join the two histories back together
//! instead of losing the source group's commits.
//!
//! Deliberately not a [`super::MemoryRef`] and never an entry of
//! [`super::MemoryFrontmatter::refs`]: a `MemoryRef` names another
//! memory by UUID at one pinned commit, while this pointer names a
//! commit RANGE on a specific PATH in another group's repository, a
//! different shape for a different purpose.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::MemoryRef;
use crate::memory::refs::InvalidCommit;

/// Where a memory's pre-move history lives.
///
/// Carried on [`super::MemoryFrontmatter::history_source`] after a
/// cross-group move; absent on every memory that has never moved
/// across groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossGroupHistoryPointer {
    /// UUID of the group the memory moved out of.
    pub source_group: Uuid,

    /// Repo-relative path (`memories/<slug>/<id>.md`) the memory
    /// occupied in the source group.
    pub source_path: String,

    /// Oldest commit touching `source_path` in the source group, at
    /// move time.
    pub first_commit: String,

    /// Newest commit touching `source_path` in the source group
    /// that still carries content, i.e. the last edit before the
    /// move deleted the file there.
    pub last_commit: String,
}

impl CrossGroupHistoryPointer {
    /// Build a pointer, rejecting a `first_commit` or `last_commit`
    /// that is not a full 40-character lowercase hex sha (see
    /// [`MemoryRef::validate_commit_shape`]).
    pub fn new(
        source_group: Uuid,
        source_path: impl Into<String>,
        first_commit: impl Into<String>,
        last_commit: impl Into<String>,
    ) -> Result<Self, InvalidCommit> {
        let first_commit = first_commit.into();
        let last_commit = last_commit.into();
        MemoryRef::validate_commit_shape(&first_commit)?;
        MemoryRef::validate_commit_shape(&last_commit)?;
        Ok(Self {
            source_group,
            source_path: source_path.into(),
            first_commit,
            last_commit,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn forty_char_hex(byte: char) -> String {
        std::iter::repeat_n(byte, 40).collect()
    }

    #[test]
    fn new_accepts_two_valid_shas() {
        let group = Uuid::now_v7();
        let pointer = CrossGroupHistoryPointer::new(
            group,
            "memories/moved-memory/0123456789.md",
            forty_char_hex('a'),
            forty_char_hex('b'),
        )
        .expect("valid shas must build");
        assert_eq!(pointer.source_group, group);
        assert_eq!(pointer.first_commit, forty_char_hex('a'));
        assert_eq!(pointer.last_commit, forty_char_hex('b'));
    }

    #[test]
    fn new_rejects_an_abbreviated_first_commit() {
        let err = CrossGroupHistoryPointer::new(
            Uuid::now_v7(),
            "memories/moved-memory/0123456789.md",
            "abc1234",
            forty_char_hex('b'),
        )
        .expect_err("abbreviated sha must be rejected");
        assert_eq!(err.input, "abc1234");
    }

    #[test]
    fn new_rejects_an_abbreviated_last_commit() {
        let err = CrossGroupHistoryPointer::new(
            Uuid::now_v7(),
            "memories/moved-memory/0123456789.md",
            forty_char_hex('a'),
            "abc1234",
        )
        .expect_err("abbreviated sha must be rejected");
        assert_eq!(err.input, "abc1234");
    }

    #[test]
    fn round_trips_through_toml() {
        let pointer = CrossGroupHistoryPointer::new(
            Uuid::now_v7(),
            "memories/moved-memory/0123456789.md",
            forty_char_hex('a'),
            forty_char_hex('b'),
        )
        .expect("valid pointer");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            history_source: CrossGroupHistoryPointer,
        }
        let rendered = toml::to_string(&Wrapper {
            history_source: pointer.clone(),
        })
        .expect("render");
        assert!(
            rendered.contains("[history_source]"),
            "rendered: {rendered}"
        );
        let parsed: Wrapper = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed.history_source, pointer);
    }
}
