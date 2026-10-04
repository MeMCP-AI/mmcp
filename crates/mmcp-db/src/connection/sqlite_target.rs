//! [`SqliteTarget`], a SQLite URL that passed every refusal rule.

use std::fmt;
use std::str::FromStr;

use sea_orm::sqlx;
use sea_orm::sqlx::sqlite::SqliteConnectOptions;
use url::{Url, form_urlencoded};

use super::at_sign_check::has_at_sign_outside_userinfo;
use super::database_url_error::DatabaseUrlError;
use super::parameter_set::ParameterSet;
use super::sqlite_parameters::{ACCEPTED_KEYS, canonical_by_case, closed_values};

/// The connection text of a SQLite database.
///
/// SQLite takes no credential, so the text is its own display.
/// A URL that carries userinfo, or an `@` outside it, is refused, since it could only be misplaced password text.
#[derive(Debug, Clone)]
pub struct SqliteTarget {
    text: String,
}

impl SqliteTarget {
    /// Applies the refusal rules to `url`, the parse of `raw`.
    pub(super) fn parse(raw: &str, url: &Url) -> Result<Self, DatabaseUrlError> {
        if !url.username().is_empty() || url.password().is_some() {
            return Err(DatabaseUrlError::SqliteUrlWithCredentials);
        }
        if has_at_sign_outside_userinfo(url) {
            return Err(DatabaseUrlError::AtSignOutsideUserinfo);
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
