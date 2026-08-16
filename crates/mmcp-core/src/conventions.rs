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

/// Header carrying the shared push-token credential `POST /sync/push`
/// requires in addition to the caller's own per-user bearer session
/// token (mmcp issue #190). Deliberately distinct from
/// `Authorization`, which the server's `AuthenticatedUser` extractor
/// already owns for per-user session verification on this same
/// route: reusing `Authorization` for the push token would make it
/// collide with the session token on the one header a request
/// carries.
///
/// Single source of truth for both ends of the request: the server
/// side re-exports this from `mmcp_server::routes::defaults` instead
/// of restating the literal, and the client side attaches it in
/// `mmcp_sync::client::SyncClient::push_version`. Both crates already
/// depend on `mmcp-core`, so the literal can never drift between the
/// header the client sends and the header the server checks.
pub const PUSH_TOKEN_HEADER: &str = "x-mmcp-push-token";

/// Build the in-repo path for a memory file: `memories/<slug>/<uuid>.md`.
/// The UUID is the canonical filename so duplicate slugs coexist as
/// sibling files under the shared slug directory.
#[must_use]
pub fn memory_path(slug: &str, id: MemoryId) -> String {
    format!("{MEMORIES_DIR}/{slug}/{id}{MEMORY_EXTENSION}")
}

/// How far [`slug_matches_filter`] walks beneath the anchor (the
/// prefix when one is set, or the implicit `memories/` root when
/// not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlugRecursion {
    /// Every descendant beneath the anchor matches, at any depth.
    Recursive,
    /// Only the anchor itself and its immediate children match; a
    /// slug two or more levels below the anchor is excluded.
    AnchorAndImmediateChildren,
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
/// - `prefix = None, recursion = Recursive` (default): every slug
///   matches.
/// - `prefix = None, recursion = AnchorAndImmediateChildren`: only
///   top-level slugs (no `/` separator) match.
/// - `prefix = Some("a/b"), recursion = Recursive`: slugs that equal
///   `"a/b"` or live underneath it match.
/// - `prefix = Some("a/b"), recursion = AnchorAndImmediateChildren`:
///   only the immediate children of the prefix and the prefix itself
///   match (so `a/b`, `a/b/c` ok; `a/b/c/d` filtered out).
///
/// Single source of truth for this filter, shared by `commands::memory` and `commands::serve`.
#[must_use]
pub fn slug_matches_filter(slug: &str, prefix: Option<&str>, recursion: SlugRecursion) -> bool {
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
    match recursion {
        SlugRecursion::Recursive => true,
        SlugRecursion::AnchorAndImmediateChildren => depth <= 1,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        use SlugRecursion::{AnchorAndImmediateChildren, Recursive};

        // No prefix, Recursive: every slug matches.
        assert!(slug_matches_filter("a", None, Recursive));
        assert!(slug_matches_filter("a/b/c", None, Recursive));
        // No prefix, AnchorAndImmediateChildren: only top-level slugs.
        assert!(slug_matches_filter("a", None, AnchorAndImmediateChildren));
        assert!(!slug_matches_filter(
            "a/b",
            None,
            AnchorAndImmediateChildren
        ));
        // Prefix match, Recursive.
        assert!(slug_matches_filter("a/b", Some("a"), Recursive));
        assert!(slug_matches_filter("a/b/c/d", Some("a/b"), Recursive));
        // Prefix match, AnchorAndImmediateChildren: only depth <= 1
        // below prefix.
        assert!(slug_matches_filter(
            "a",
            Some("a"),
            AnchorAndImmediateChildren
        ));
        assert!(slug_matches_filter(
            "a/b",
            Some("a"),
            AnchorAndImmediateChildren
        ));
        assert!(!slug_matches_filter(
            "a/b/c",
            Some("a"),
            AnchorAndImmediateChildren
        ));
        // Prefix mismatch.
        assert!(!slug_matches_filter("ab", Some("a"), Recursive));
        assert!(!slug_matches_filter("b/a", Some("a"), Recursive));
    }
}
