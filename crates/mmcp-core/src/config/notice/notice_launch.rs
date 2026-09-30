//! [`NoticeLaunch`], the launch values of every notice key.

use super::LaunchValues;
use crate::config::ConfigKey;

/// What a serving process was launched with, per key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoticeLaunch {
    /// Launch values of `notice.md.project`.
    pub md_project: LaunchValues,
    /// Launch values of `notice.md.user`.
    pub md_user: LaunchValues,
}

impl NoticeLaunch {
    /// Launch values of `key`.
    #[must_use]
    pub const fn for_key(&self, key: ConfigKey) -> LaunchValues {
        match key {
            ConfigKey::NoticeMdProject => self.md_project,
            ConfigKey::NoticeMdUser => self.md_user,
        }
    }
}
