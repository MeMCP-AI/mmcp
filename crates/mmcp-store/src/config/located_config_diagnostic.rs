//! [`LocatedConfigDiagnostic`], a configuration diagnostic with the file it was found in.

use std::path::PathBuf;

use mmcp_core::config::ConfigDiagnostic;

/// A [`ConfigDiagnostic`] attached to the configuration file whose load produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedConfigDiagnostic {
    /// File the diagnostic was found in.
    pub path: PathBuf,
    /// What the loader tolerated in that file.
    pub diagnostic: ConfigDiagnostic,
}
