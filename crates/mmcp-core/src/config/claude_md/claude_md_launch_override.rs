//! [`ClaudeMdLaunchOverride`], the suggestion values a serving process was launched with.

use super::ClaudeMdSuggestion;

/// The two launch layers of the CLAUDE.md suggestion, collected once by the argument parser.
/// They act as a per-registration global switch: they rank below both per-project layers and above the user's global setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaudeMdLaunchOverride {
    /// Value of the launch flag.
    pub flag: Option<ClaudeMdSuggestion>,
    /// Value of the launch environment variable.
    pub environment: Option<ClaudeMdSuggestion>,
}
