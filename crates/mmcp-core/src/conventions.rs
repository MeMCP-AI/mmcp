//! Project-wide conventions and constants.
//!
//! Single source of truth for string literals, path components,
//! and default values used across the workspace. Any crate that
//! needs one of these values imports from here instead of
//! hardcoding the string.

use uuid::Uuid;

/// Default branch name used for all group repositories.
pub const MAIN_BRANCH: &str = "main";

/// Full ref path for the main branch.
pub const MAIN_BRANCH_REF: &str = "refs/heads/main";

/// Remote-tracking ref path that fetch-without-apply writes into,
/// so the local `refs/heads/main` stays put and callers can compare
/// before fast-forwarding. Mirrors git's `origin/<branch>` layout.
pub const MAIN_REMOTE_TRACKING_REF: &str = "refs/remotes/origin/main";

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

/// Build the in-repo path for a memory file: `memories/<slug>/<uuid>.md`.
/// The UUID is the canonical filename so duplicate slugs coexist as
/// sibling files under the shared slug directory.
#[must_use]
pub fn memory_path(slug: &str, id: Uuid) -> String {
    format!("{MEMORIES_DIR}/{slug}/{id}{MEMORY_EXTENSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_path_builds_two_level_path() {
        let id = Uuid::parse_str("0196e5bb-a000-7000-8000-000000000001").unwrap();
        assert_eq!(
            memory_path("uuidify-memories", id),
            "memories/uuidify-memories/0196e5bb-a000-7000-8000-000000000001.md"
        );
    }
}
