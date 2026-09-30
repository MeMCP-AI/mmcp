//! The CLAUDE.md suggestion setting and its resolution across configuration layers.
//!
//! One lenient `claude_md` table type is reused at project level, at user level and per project in the user config.
//! [`ClaudeMdLayers`] resolves the per-layer values into the effective suggestion.

mod claude_md_launch_override;
mod claude_md_layers;
mod claude_md_resolution;
mod claude_md_set_outcome;
mod claude_md_setting_error;
mod claude_md_source;
mod claude_md_suggestion;
mod claude_md_table;

pub use claude_md_launch_override::ClaudeMdLaunchOverride;
pub use claude_md_layers::ClaudeMdLayers;
pub use claude_md_resolution::ClaudeMdResolution;
pub use claude_md_set_outcome::ClaudeMdSetOutcome;
pub use claude_md_setting_error::ClaudeMdSettingError;
pub use claude_md_source::ClaudeMdSource;
pub use claude_md_suggestion::ClaudeMdSuggestion;
pub use claude_md_table::{CLAUDE_MD_TABLE_KEY, ClaudeMdTable, PROJECT_FILE_SUGGESTION_KEY};
