//! [`ClaudeMdSource`], the layer a resolved suggestion comes from.

/// Layer that decided the effective suggestion, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeMdSource {
    /// The user's entry for this project in `~/.mmcp/config.toml`.
    UserProject,
    /// The project's `.mmcp.toml`.
    Project,
    /// The launch flag of the serving process.
    Flag,
    /// The launch environment variable of the serving process.
    Environment,
    /// The user's setting for every project in `~/.mmcp/config.toml`.
    User,
    /// No layer sets a value: the built-in default.
    Default,
}

impl ClaudeMdSource {
    /// Wire spelling of this source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserProject => "user_project",
            Self::Project => "project",
            Self::Flag => "flag",
            Self::Environment => "environment",
            Self::User => "user",
            Self::Default => "default",
        }
    }
}
