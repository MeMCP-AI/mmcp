//! [`DatabaseUrlError`], why a database URL is refused.

use thiserror::Error;

use super::postgres_parameters::accepted_keys_text;

/// Why a database URL is refused.
///
/// No variant holds text of the URL beyond its scheme.
/// A refusal must not echo a password that the URL grammar placed where it does not expect one.
/// Every text names only constants of mmcp, positions and counts.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DatabaseUrlError {
    /// The text is not a URL.
    #[error("the database URL cannot be parsed")]
    Unparsable {
        #[source]
        source: url::ParseError,
    },

    /// The scheme names no driver.
    #[error(
        "the database URL names the scheme `{scheme}`, which no driver serves; use `postgres`, `postgresql` or `sqlite`"
    )]
    UnsupportedScheme { scheme: String },

    /// A Postgres URL without `//` after its scheme puts the rest of the text into the database name.
    #[error("a postgres URL has no `//` after its scheme; write `postgres://`")]
    PostgresWithoutAuthority,

    /// A Postgres URL carries no fragment, so a `#` can only be misplaced password text.
    #[error(
        "a postgres URL has a `#` part; percent-encode a `#` of the password as `%23`, and avoid `#` in `POSTGRES_PASSWORD` of the compose"
    )]
    FragmentPresent,

    /// The userinfo of a URL ends at its last `@` before the first `/`, `?` or `#`, so an `@` further on follows misplaced password text.
    #[error(
        "the database URL has an `@` outside its userinfo; percent-encode it as `%40`, give a database name holding one through `dbname=` with `%40`, and avoid `/`, `?` and `#` in a password, in `POSTGRES_PASSWORD` of the compose too"
    )]
    AtSignOutsideUserinfo,

    /// No parameter of a Postgres URL is valueless, so its text could be anything.
    #[error(
        "a parameter of the database URL has no `=`; give it a value, or percent-encode `&` as `%26`"
    )]
    ParameterWithoutValue,

    /// A parameter that equals an accepted one except for its case.
    #[error(
        "parameter {position} of {count} of the database URL must be written `{canonical}`, in lowercase"
    )]
    ParameterWrongCase {
        position: usize,
        count: usize,
        canonical: &'static str,
    },

    /// A parameter that sqlx would log at warn with its value and ignore.
    #[error(
        "parameter {position} of {count} of the database URL is not accepted; the accepted ones are {}",
        accepted_keys_text()
    )]
    UnrecognizedParameter { position: usize, count: usize },

    /// A value outside the closed set of an accepted parameter, which sqlx would quote in its error.
    #[error(
        "the value of the `{key}` parameter of the database URL must be one of {}",
        accepted.join(", ")
    )]
    InvalidParameterValue {
        key: &'static str,
        accepted: &'static [&'static str],
    },

    /// SQLite takes no credential, so userinfo can only be misplaced text.
    #[error("a sqlite URL takes no user name and no password")]
    SqliteUrlWithCredentials,
}
