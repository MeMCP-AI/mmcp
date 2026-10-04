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
    /// Whether this layer belongs to the serving process, not to a configuration file.
    #[must_use]
    pub const fn is_launch(self) -> bool {
        matches!(self, Self::Flag | Self::Environment)
    }

    /// Whether this layer outranks both launch layers, so a value found here holds whatever the serving process was launched with.
    #[must_use]
    pub const fn outranks_launch(self) -> bool {
        matches!(self, Self::Local | Self::Project)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [NoticeSource; 6] = [
        NoticeSource::Local,
        NoticeSource::Project,
        NoticeSource::Flag,
        NoticeSource::Environment,
        NoticeSource::User,
        NoticeSource::Default,
    ];

    #[test]
    fn exactly_the_flag_and_the_environment_are_launch_layers() {
        let launch: Vec<NoticeSource> = ALL.into_iter().filter(|s| s.is_launch()).collect();
        assert_eq!(launch, [NoticeSource::Flag, NoticeSource::Environment]);
    }

    #[test]
    fn exactly_the_local_and_project_layers_outrank_the_launch_layers() {
        let outranking: Vec<NoticeSource> =
            ALL.into_iter().filter(|s| s.outranks_launch()).collect();
        assert_eq!(outranking, [NoticeSource::Local, NoticeSource::Project]);
    }
}
