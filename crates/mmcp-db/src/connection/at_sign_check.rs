//! The check for an `@` that sits outside the userinfo of a Postgres URL.

use percent_encoding::percent_decode_str;

/// Whether the raw text of a Postgres URL holds an `@` after its authority.
///
/// The userinfo of a URL ends at its last `@` before the first `/`, `?` or `#`.
/// Password text that holds one of those characters pushes the real userinfo `@` into the path or the query.
/// The text is read as it was written, because the URL parser removes dot segments from the path, which erases an `@` that sits in one.
/// The path is checked raw and percent-decoded, since a driver decodes it into the database name.
/// The query is checked raw, since `%40` is how a value holds an `@`.
pub(super) fn has_at_sign_outside_userinfo(raw: &str) -> bool {
    // The URL parser drops tabs and line breaks from its input, so they are dropped here too.
    let text: String = raw
        .chars()
        .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
        .collect();
    let Some((_, after_scheme)) = text.split_once("://") else {
        return false;
    };
    let Some(authority_end) = after_scheme.find(['/', '?', '#']) else {
        return false;
    };
    let tail = &after_scheme[authority_end..];
    if tail.contains('@') {
        return true;
    }
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    percent_decode_str(path).decode_utf8_lossy().contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_at_sign_in_a_dot_segment_is_found() {
        for text in [
            "postgres://u:123/SECRET/tail@db/../app",
            "postgres://u:123/SECRET/tail@db/%2e%2e/app",
            "postgres://u:123/SECRET/tail@db/%2E%2E/app",
            "postgres://u:123/SECRET/tail@db/./app",
            "postgres://u:123/SECRET/tail@db/..",
        ] {
            assert!(has_at_sign_outside_userinfo(text), "{text}");
        }
    }

    #[test]
    fn tabs_and_line_breaks_do_not_hide_the_authority_end() {
        assert!(has_at_sign_outside_userinfo(
            "post\tgres://u:1\n23/SECRET/tail\r@db/../app"
        ));
    }

    #[test]
    fn the_userinfo_at_sign_is_not_outside_it() {
        for text in [
            "postgres://mmcp:pw@db/app",
            "postgres://user@server:pw@host/db",
            "postgres:///db",
            "postgres://db",
            "postgres://db/app?sslmode=require",
        ] {
            assert!(!has_at_sign_outside_userinfo(text), "{text}");
        }
    }
}
