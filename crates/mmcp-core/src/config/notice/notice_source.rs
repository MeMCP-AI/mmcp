//! [`NoticeSource`], the layer a resolved notice value comes from.

/// Layer that decided the effective value, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoticeSource {
    /// `.mmcp.local.toml`.
    Local,
    /// `.mmcp.toml`.
    Project,
    /// The launch flag of the serving process.
    Flag,
    /// The launch environment variable of the serving process.
    Environment,
    /// `~/.mmcp/config.toml`.
    User,
    /// No layer sets a value: the default of the key.
    Default,
}

impl NoticeSource {
    /// Wire spelling of this source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Project => "project",
            Self::Flag => "flag",
            Self::Environment => "environment",
            Self::User => "user",
            Self::Default => "default",
        }
    }
}
