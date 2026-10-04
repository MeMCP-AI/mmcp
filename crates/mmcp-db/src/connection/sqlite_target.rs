//! [`SqliteTarget`], a SQLite URL that passed every refusal rule.

use std::fmt;
use std::str::FromStr;

use sea_orm::sqlx;
use sea_orm::sqlx::sqlite::SqliteConnectOptions;
use url::form_urlencoded;

use super::database_url_error::DatabaseUrlError;
use super::parameter_set::ParameterSet;
use super::sqlite_parameters::{ACCEPTED_KEYS, canonical_by_case, closed_values};

/// The connection text of a SQLite database.
///
/// SQLite takes no credential, so the text is its own display.
/// sqlx-sqlite reads the text after the scheme as a file name, percent-decoded.
/// A raw `@` is refused, since it can be the userinfo of a misplaced credential, and `%40` is how a file name holds one.
#[derive(Debug, Clone)]
pub struct SqliteTarget {
    text: String,
}

impl SqliteTarget {
    /// Applies the refusal rules to `raw`, which the URL parser has accepted.
    pub(super) fn parse(raw: &str) -> Result<Self, DatabaseUrlError> {
        if raw.contains('@') {
            return Err(DatabaseUrlError::SqliteAtSignInFileName);
        }
        if raw.contains('#') {
            return Err(DatabaseUrlError::SqliteFragmentPresent);
        }
        check_parameters(raw)?;
        Ok(Self {
            text: raw.to_owned(),
        })
    }

    /// The sqlx options that the text spells, read by sqlx's own parser.
    pub(super) fn connect_options(&self) -> Result<SqliteConnectOptions, sqlx::Error> {
        SqliteConnectOptions::from_str(&self.text)
    }
}

impl fmt::Display for SqliteTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

/// Checks the query of `raw` as sqlx-sqlite reads it: split at the first `?`, then form-decoded.
///
/// A key outside the accepted set is refused, and so is a value outside the closed set of `mode`, `cache` and `immutable`.
fn check_parameters(raw: &str) -> Result<(), DatabaseUrlError> {
    let Some((_, query)) = raw.split_once('?') else {
        return Ok(());
    };
    let pairs: Vec<_> = form_urlencoded::parse(query.as_bytes()).collect();
    let count = pairs.len();
    for (index, (key, value)) in pairs.iter().enumerate() {
        let position = index + 1;
        let Some(accepted) = ACCEPTED_KEYS.iter().find(|accepted| **accepted == &**key) else {
            return Err(match canonical_by_case(key) {
                Some(canonical) => DatabaseUrlError::ParameterWrongCase {
                    set: ParameterSet::Sqlite,
                    position,
                    count,
                    canonical,
                },
                None => DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Sqlite,
                    position,
                    count,
                },
            });
        };
        if let Some(values) = closed_values(accepted)
            && !values.iter().any(|allowed| *allowed == &**value)
        {
            return Err(DatabaseUrlError::InvalidParameterValue {
                key: accepted,
                accepted: values,
            });
        }
    }
    Ok(())
}
