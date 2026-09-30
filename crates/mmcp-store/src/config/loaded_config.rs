//! [`LoadedConfig`], a configuration loaded with the diagnostics of its load.

use std::path::Path;

use mmcp_core::config::ConfigDiagnostic;

use super::LocatedConfigDiagnostic;

/// A loaded configuration and what the loader tolerated while reading it.
#[derive(Debug, Clone)]
pub struct LoadedConfig<T> {
    /// The loaded configuration.
    pub config: T,
    /// Every tolerated mistake, each attached to its file.
    pub diagnostics: Vec<LocatedConfigDiagnostic>,
}

impl<T> LoadedConfig<T> {
    /// Attach `path` to every diagnostic a loader returned for `config`.
    pub(crate) fn located(config: T, path: &Path, diagnostics: Vec<ConfigDiagnostic>) -> Self {
        let diagnostics = diagnostics
            .into_iter()
            .map(|diagnostic| LocatedConfigDiagnostic {
                path: path.to_path_buf(),
                diagnostic,
            })
            .collect();
        Self {
            config,
            diagnostics,
        }
    }

    /// The configuration, after logging every diagnostic as a warning.
    /// For callers that do not surface diagnostics themselves: nothing the loader tolerated is silent.
    #[must_use]
    pub fn into_config_logging_diagnostics(self) -> T {
        for located in &self.diagnostics {
            tracing::warn!(
                path = %located.path.display(),
                diagnostic = %located.diagnostic,
                "configuration diagnostic"
            );
        }
        self.config
    }
}
