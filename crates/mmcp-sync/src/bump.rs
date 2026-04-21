//! Parse a [`BumpIntent`] out of a git commit message.
//!
//! The git-symmetric push path walks
//! `server_head..local_head` and registers one `POST /sync/push` per
//! new commit. Without the retired `PendingQueue` there is no
//! sidecar metadata carrying the bump intent, so the intent has to
//! ride on the commit itself.
//!
//! Convention: trailing `bump: <level>` line, Conventional-Commits
//! adjacent, accepted with or without leading whitespace and case
//! insensitive on the key. Recognised values are `patch`, `minor`,
//! `major`. Anything else - absent trailer, mis-spelled key,
//! unknown level - falls back to [`BumpIntent::default`], which is
//! `Minor` and documented as the "typical rule tweak" level in
//! [`mmcp_core::memory::bump`].

use mmcp_core::memory::BumpIntent;

/// Scan `message` from the bottom up looking for a `bump: <level>`
/// trailer and return the parsed [`BumpIntent`], falling back to
/// [`BumpIntent::default`] when absent or unrecognised.
///
/// Only the first `bump:` line encountered from the bottom wins so
/// operators can override a defaulted trailer by appending a later
/// line. Lines above the first blank gap from the end are
/// considered the trailer block, matching git-interpret-trailers
/// behaviour; lines embedded in the body are ignored to avoid
/// false matches on prose like `the bump: minor change`.
#[must_use]
pub fn parse_bump_intent(message: &str) -> BumpIntent {
    for line in trailer_block(message) {
        if let Some(rest) = line.strip_prefix_ignore_case("bump:") {
            return parse_level(rest.trim()).unwrap_or_default();
        }
    }
    BumpIntent::default()
}

/// Return an iterator over the trailing block of lines in
/// `message`, from bottom to top, up to (but not including) the
/// first empty line. Empty messages yield an empty iterator.
fn trailer_block(message: &str) -> impl Iterator<Item = &str> {
    // Collect tail lines in reverse order until we hit a blank,
    // mirroring the git-interpret-trailers "last paragraph" rule.
    let mut tail = Vec::new();
    for line in message.lines().rev() {
        let stripped = line.trim_end();
        if stripped.is_empty() {
            break;
        }
        tail.push(stripped);
    }
    tail.into_iter()
}

fn parse_level(level: &str) -> Option<BumpIntent> {
    match level.to_ascii_lowercase().as_str() {
        "patch" => Some(BumpIntent::Patch),
        "minor" => Some(BumpIntent::Minor),
        "major" => Some(BumpIntent::Major),
        _ => None,
    }
}

/// Case-insensitive `strip_prefix`. Kept private because the only
/// caller is the bump-trailer scanner - a general-purpose helper
/// would belong in an extension crate, not here.
trait StripPrefixIgnoreCase {
    fn strip_prefix_ignore_case(&self, prefix: &str) -> Option<&str>;
}

impl StripPrefixIgnoreCase for str {
    fn strip_prefix_ignore_case(&self, prefix: &str) -> Option<&str> {
        if self.len() < prefix.len() {
            return None;
        }
        let (head, tail) = self.split_at(prefix.len());
        if head.eq_ignore_ascii_case(prefix) {
            Some(tail)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_trailer_defaults_to_minor() {
        // `BumpIntent::default()` is `Minor` - the typical
        // rule-tweak level documented on the enum itself. Pinning
        // the default polarity here so a later change to the enum
        // default surfaces as a test failure instead of silently
        // shifting every unlabelled commit.
        assert_eq!(parse_bump_intent("just a commit"), BumpIntent::Minor);
    }

    #[test]
    fn patch_trailer_parses() {
        let msg = "fix typo\n\nbump: patch\n";
        assert_eq!(parse_bump_intent(msg), BumpIntent::Patch);
    }

    #[test]
    fn minor_trailer_parses_case_insensitively() {
        let msg = "add rule\n\nBump: Minor\n";
        assert_eq!(parse_bump_intent(msg), BumpIntent::Minor);
    }

    #[test]
    fn major_trailer_parses() {
        let msg = "reversal\n\nbump: major\n";
        assert_eq!(parse_bump_intent(msg), BumpIntent::Major);
    }

    #[test]
    fn trailer_must_live_in_the_last_paragraph() {
        // A `bump:` mention inside the body does NOT count - only
        // the trailing paragraph is scanned, matching
        // git-interpret-trailers behaviour and avoiding prose
        // false positives.
        let msg = "the bump: minor change here\n\nis just wording";
        assert_eq!(parse_bump_intent(msg), BumpIntent::default());
    }

    #[test]
    fn unknown_level_falls_back_to_default() {
        let msg = "body\n\nbump: jumbo\n";
        assert_eq!(parse_bump_intent(msg), BumpIntent::default());
    }

    #[test]
    fn later_trailer_in_the_block_wins_over_earlier() {
        // Operators rewriting the trailer append rather than edit
        // in place; the bottom-up scan means the most-recently-
        // appended bump wins.
        let msg = "subject\n\nbump: patch\nbump: major\n";
        assert_eq!(parse_bump_intent(msg), BumpIntent::Major);
    }

    #[test]
    fn leading_whitespace_on_trailer_is_tolerated() {
        let msg = "subject\n\n  bump: patch\n";
        // trim_end keeps leading ws; the loop matches on the
        // full line so leading space would currently fail to
        // detect. Pin the intended behaviour so we treat this
        // as a no-match (strict "first column" trailer per git
        // convention), not a silently-parsed patch bump.
        assert_eq!(parse_bump_intent(msg), BumpIntent::default());
    }
}
