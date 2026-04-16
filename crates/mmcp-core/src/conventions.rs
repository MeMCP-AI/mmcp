//! Project-wide conventions and constants.
//!
//! Single source of truth for string literals, path components,
//! and default values used across the workspace. Any crate that
//! needs one of these values imports from here instead of
//! hardcoding the string.

/// Default branch name used for all group repositories.
pub const MAIN_BRANCH: &str = "main";

/// Full ref path for the main branch.
pub const MAIN_BRANCH_REF: &str = "refs/heads/main";

/// Directory inside a group repository that holds memory files.
pub const MEMORIES_DIR: &str = "memories";

/// File extension for memory markdown files.
pub const MEMORY_EXTENSION: &str = ".md";

/// Default author name for commits made by mmcp itself
/// (init, import, manifest writes).
pub const MMCP_AUTHOR_NAME: &str = "mmcp";

/// Default author email for commits made by mmcp itself.
pub const MMCP_AUTHOR_EMAIL: &str = "mmcp@mmcp.invalid";

/// The 40-character zero hash used to represent "no commit yet".
pub const ZERO_COMMIT: &str = "0000000000000000000000000000000000000000";

/// Build the in-repo path for a memory file: `memories/<slug>.md`.
#[must_use]
pub fn memory_path(slug: &str) -> String {
    format!("{MEMORIES_DIR}/{slug}{MEMORY_EXTENSION}")
}
