//! [`ConfigWriteOutcome`], what a set or unset did.

use std::path::PathBuf;

use mmcp_core::config::{ConfigKey, ConfigScope, NoticeResolution, NoticeValue};

use super::ConfigOpError;

/// Result of a set or unset of a configuration key at one scope.
/// The write is done whatever `resolution` holds.
#[derive(Debug)]
pub struct ConfigWriteOutcome {
    /// The key written.
    pub key: ConfigKey,
    /// The scope written.
    pub scope: ConfigScope,
    /// The value set, `None` for an unset.
    pub value: Option<NoticeValue>,
    /// Whether the stored value differs from before, so whether the file was written.
    pub changed: bool,
    /// The file that holds the scope.
    pub file: PathBuf,
    /// The global git excludes file that received the local file's pattern, when this write appended it.
    pub excluded_in: Option<PathBuf>,
    /// The effective value of the key after the write, with its source and every layer.
    /// An error when a file of another scope cannot be read, which leaves the effective value unknown.
    pub resolution: Result<NoticeResolution, ConfigOpError>,
}

#[cfg(test)]
impl ConfigWriteOutcome {
    /// The resolution of a test whose files all load.
    #[allow(clippy::expect_used)]
    pub(super) fn resolved(&self) -> &NoticeResolution {
        self.resolution
            .as_ref()
            .expect("every file of the scenario loads")
    }
}
