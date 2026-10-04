//! [`SqliteTarget`], a SQLite URL that passed every refusal rule.

use std::fmt;
use std::str::FromStr;

use sea_orm::sqlx;
use sea_orm::sqlx::sqlite::SqliteConnectOptions;
use url::Url;

use super::at_sign_check::has_at_sign_outside_userinfo;
use super::database_url_error::DatabaseUrlError;

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
