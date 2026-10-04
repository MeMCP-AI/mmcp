//! The check for an `@` that sits outside the userinfo of a database URL.

use percent_encoding::percent_decode_str;
use url::Url;

/// Whether `url` holds an `@` in its path, decoded, or in its raw query.
///
/// The userinfo of a URL ends at its last `@` before the first `/`, `?` or `#`.
/// Password text that holds one of those characters pushes the real userinfo `@` into the path or the query.
/// The path is checked after percent-decoding, since a driver decodes it into the database name.
/// The query is checked raw, since `%40` is how a value holds an `@`.
pub(super) fn has_at_sign_outside_userinfo(url: &Url) -> bool {
    let path_has_it = percent_decode_str(url.path())
        .decode_utf8_lossy()
        .contains('@');
    path_has_it || url.query().is_some_and(|query| query.contains('@'))
}
