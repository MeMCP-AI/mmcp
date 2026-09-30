//! [`MdNoticeConfig`], the `[notice.md]` table.

use serde::{Deserialize, Serialize};

use super::NoticeValue;

/// Keys governing the notices that propose the mmcp block for a CLAUDE.md file.
/// Each key governs the notices of its own file only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MdNoticeConfig {
    /// `notice.md.project`: the notices for the project's CLAUDE.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<NoticeValue>,

    /// `notice.md.user`: the notices for the user-level `~/.claude/CLAUDE.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<NoticeValue>,
}

impl MdNoticeConfig {
    /// Whether no key is set, so the table is left out of the file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.user.is_none()
    }
}
