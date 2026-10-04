//! [`ConfigOutcome`], what a `config` command did.

use mmcp_core::config::{ConfigKey, NoticeResolution};

use super::ConfigWriteOutcome;

/// Result of one [`super::ConfigCommand`].
#[derive(Debug)]
pub enum ConfigOutcome {
    /// A get: the key with the value of every layer, the effective value and its source.
    Read {
        /// The key read.
        key: ConfigKey,
        /// Its resolution.
        resolution: NoticeResolution,
    },
    /// A set or an unset.
    Written(ConfigWriteOutcome),
}
