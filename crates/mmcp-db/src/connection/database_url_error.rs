//! [`DatabaseUrlError`], why a database URL is refused.

use thiserror::Error;

use super::parameter_set::ParameterSet;

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
    /// A bare `postgres://` prefix would turn the database name into a host, so the remedy keeps the database in the path.
    #[error(
        "a postgres URL has no `//` after its scheme; write `postgres://HOST/DB`, or `postgres:///DB?host=SOCKET_DIRECTORY` to keep the database name and use a socket directory or the default host"
    )]
    PostgresWithoutAuthority,

    /// A Postgres URL carries no fragment, so a `#` can only be misplaced password text.
    #[error(
        "a postgres URL has a `#` part; percent-encode a `#` of the password as `%23`, and avoid `#` in `POSTGRES_PASSWORD` of the compose"
    )]
    FragmentPresent,

    /// A SQLite URL carries no fragment, and sqlx-sqlite reads a `#` as part of the file name, so a `#` can only be misplaced text.
    #[error("a sqlite URL has a `#`; percent-encode a `#` of the file name as `%23`")]
    SqliteFragmentPresent,

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
        "parameter {position} of {count} of the {set} URL must be written `{canonical}`, in lowercase"
    )]
    ParameterWrongCase {
        set: ParameterSet,
        position: usize,
        count: usize,
        canonical: &'static str,
    },

    /// A parameter that the driver would log with its value and ignore, or refuse at connect.
    #[error(
        "parameter {position} of {count} of the {set} URL is not accepted; the accepted ones are {}",
        set.accepted_text()
    )]
    UnrecognizedParameter {
        set: ParameterSet,
        position: usize,
        count: usize,
    },

    /// A value outside the closed set of an accepted parameter, which the driver would quote in its error.
    #[error(
        "the value of the `{key}` parameter of the database URL must be one of {}",
        accepted.join(", ")
    )]
    InvalidParameterValue {
        key: &'static str,
        accepted: &'static [&'static str],
    },

    /// SQLite takes no credential, so a raw `@` can only be a file name character or the userinfo of misplaced text.
    /// sqlx-sqlite decodes the file name, so `%40` is how it holds an `@`.
    #[error("a sqlite URL has an `@`; percent-encode an `@` of the file name as `%40`")]
    SqliteAtSignInFileName,
}
