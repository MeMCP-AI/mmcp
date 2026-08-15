//! Project-wide conventions and constants.
//!
//! Single source of truth for string literals, path components,
//! and default values used across the workspace. Any crate that
//! needs one of these values imports from here instead of
//! hardcoding the string.

use crate::id::MemoryId;

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
pub fn memory_path(slug: &str, id: MemoryId) -> String {
    format!("{MEMORIES_DIR}/{slug}/{id}{MEMORY_EXTENSION}")
}

/// Does `slug` belong in a listing constrained to `prefix` and the
/// recursion mode?
///
/// `depth` is measured from the *anchor*: the prefix when one is
/// set, or the implicit `memories/` root when not. The anchor itself
/// sits at depth 0; a top-level slug like `feedback` is depth 1 from
/// the root, and one level below a prefix is depth 1 from the
/// prefix.
///
/// - `prefix = None, recursive = true` (default): every slug
///   matches.
/// - `prefix = None, recursive = false`: only top-level slugs (no
///   `/` separator) match.
/// - `prefix = Some("a/b"), recursive = true`: slugs that equal
///   `"a/b"` or live underneath it match.
/// - `prefix = Some("a/b"), recursive = false`: only the immediate
///   children of the prefix and the prefix itself match (so `a/b`,
///   `a/b/c` ok; `a/b/c/d` filtered out).
///
/// Single source of truth for this filter: previously duplicated
/// between `mmcp-client`'s `commands::memory` and `commands::serve`,
/// each with its own copy and no shared test coverage
/// (`slug_matches_filter_truth_table_agrees_across_callers`, below,
/// closes that gap).
#[must_use]
pub fn slug_matches_filter(slug: &str, prefix: Option<&str>, recursive: bool) -> bool {
    let depth = match prefix {
        None | Some("") | Some("/") => slug.split('/').count(),
        Some(p) => {
            if slug == p {
                0
            } else if let Some(rest) = slug.strip_prefix(p)
                && let Some(suffix) = rest.strip_prefix('/')
            {
                suffix.split('/').count()
            } else {
                return false;
            }
        }
    };
    if recursive { true } else { depth <= 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_path_builds_two_level_path() {
        let id = MemoryId::from_uuid(
            uuid::Uuid::parse_str("0196e5bb-a000-7000-8000-000000000001").unwrap(),
        );
        assert_eq!(
            memory_path("uuidify-memories", id),
            "memories/uuidify-memories/0196e5bb-a000-7000-8000-000000000001.md"
        );
    }

    /// Truth table migrated from `mmcp-client`'s `commands::serve`
    /// (`slug_matches_filter_truth_table`). Exercising the single
    /// mmcp-core implementation here means a regression is provable
    /// to affect every caller (`commands::memory`, `commands::serve`)
    /// at once, which two independently-copied local functions never
    /// guaranteed.
    #[test]
    fn slug_matches_filter_truth_table_agrees_across_callers() {
        // No prefix, recursive=true: every slug matches.
        assert!(slug_matches_filter("a", None, true));
        assert!(slug_matches_filter("a/b/c", None, true));
        // No prefix, recursive=false: only top-level slugs.
        assert!(slug_matches_filter("a", None, false));
        assert!(!slug_matches_filter("a/b", None, false));
        // Prefix match, recursive=true.
        assert!(slug_matches_filter("a/b", Some("a"), true));
        assert!(slug_matches_filter("a/b/c/d", Some("a/b"), true));
        // Prefix match, recursive=false: only depth <= 1 below
        // prefix.
        assert!(slug_matches_filter("a", Some("a"), false));
        assert!(slug_matches_filter("a/b", Some("a"), false));
        assert!(!slug_matches_filter("a/b/c", Some("a"), false));
        // Prefix mismatch.
        assert!(!slug_matches_filter("ab", Some("a"), true));
        assert!(!slug_matches_filter("b/a", Some("a"), true));
    }
}
