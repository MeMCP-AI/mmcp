//! [`PostgresTarget`], a Postgres URL that passed every refusal rule.

use std::fmt;
use std::str::FromStr;

use percent_encoding::percent_decode_str;
use sea_orm::sqlx;
use sea_orm::sqlx::postgres::PgConnectOptions;
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use super::at_sign_check::has_at_sign_outside_userinfo;
use super::database_url_error::DatabaseUrlError;
use super::postgres_parameters::{
    SSL_MODE_KEYS, SSL_MODE_VALUES, accepted_constant, canonical_by_case,
};
use crate::error::DbError;

/// The connection text of a Postgres database, with the display that is safe to show.
///
/// The text is held in a secret, so a Debug print shows a redaction marker.
/// The display holds the user name, host, port and path, and the names of the parameters, never a parameter value.
#[derive(Debug, Clone)]
pub struct PostgresTarget {
    display: String,
    connection_text: SecretString,
}

impl PostgresTarget {
    /// Applies the refusal rules to `url`, the parse of `raw`.
    pub(super) fn parse(raw: &str, url: &Url) -> Result<Self, DatabaseUrlError> {
        if !url.has_authority() {
            return Err(DatabaseUrlError::PostgresWithoutAuthority);
        }
        if url.fragment().is_some() {
            return Err(DatabaseUrlError::FragmentPresent);
        }
        if has_at_sign_outside_userinfo(url) {
            return Err(DatabaseUrlError::AtSignOutsideUserinfo);
        }
        if url.query().is_some_and(has_valueless_entry) {
            return Err(DatabaseUrlError::ParameterWithoutValue);
        }
        let parameters = checked_parameters(url)?;
        Ok(Self {
            display: display_of(url, &parameters),
            connection_text: SecretString::from(raw),
        })
    }

    /// The one place the connection text leaves its secret.
    fn exposed_text(&self) -> &str {
        self.connection_text.expose_secret()
    }

    /// The sqlx options that the connection text spells, read by sqlx's own parser.
    pub(super) fn connect_options(&self) -> Result<PgConnectOptions, sqlx::Error> {
        PgConnectOptions::from_str(self.exposed_text())
    }

    /// The value-free error for a configuration error that sqlx raised while connecting.
    ///
    /// sqlx raises only the SASLprep refusals of the user name and of the password there.
    /// Its error quotes the rejected character, so the source is dropped and the two are told apart by the same check.
    pub(super) fn explain_configuration_error(&self) -> DbError {
        let Ok(url) = Url::parse(self.exposed_text()) else {
            return DbError::ConnectionConfigurationRejected;
        };
        let password = url
            .password()
            .map(decoded)
            .or_else(|| query_value(&url, "password"));
        if password.is_some_and(|password| stringprep::saslprep(&password).is_err()) {
            return DbError::PasswordRejectedBySaslprep;
        }
        let user = Some(url.username())
            .filter(|user| !user.is_empty())
            .map(decoded)
            .or_else(|| query_value(&url, "user"));
        if user.is_some_and(|user| stringprep::saslprep(&user).is_err()) {
            return DbError::UsernameRejectedBySaslprep;
        }
        DbError::ConnectionConfigurationRejected
    }
}

impl fmt::Display for PostgresTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display)
    }
}

/// Whether the raw `query` holds a non-empty entry without `=`.
fn has_valueless_entry(query: &str) -> bool {
    query
        .split('&')
        .any(|entry| !entry.is_empty() && !entry.contains('='))
}

/// The percent-decoded text of a URL component.
fn decoded(component: &str) -> String {
    percent_decode_str(component)
        .decode_utf8_lossy()
        .into_owned()
}

/// The decoded value of the query parameter `key`, when `url` holds it.
fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

/// Checks every query parameter and returns the constants they are shown as, in order and without repeats.
fn checked_parameters(url: &Url) -> Result<Vec<&'static str>, DatabaseUrlError> {
    let count = url.query_pairs().count();
    let mut shown: Vec<&'static str> = Vec::new();
    for (index, (key, value)) in url.query_pairs().enumerate() {
        let position = index + 1;
        let Some(constant) = accepted_constant(&key) else {
            return Err(match canonical_by_case(&key) {
                Some(canonical) => DatabaseUrlError::ParameterWrongCase {
                    position,
                    count,
                    canonical,
                },
                None => DatabaseUrlError::UnrecognizedParameter { position, count },
            });
        };
        if SSL_MODE_KEYS.contains(&constant) {
            check_ssl_mode_value(constant, &value)?;
        }
        if !shown.contains(&constant) {
            shown.push(constant);
        }
    }
    Ok(shown)
}

/// Refuses an SSL mode outside sqlx's closed set, whose error would quote the value.
fn check_ssl_mode_value(key: &'static str, value: &str) -> Result<(), DatabaseUrlError> {
    let lowered = value.to_ascii_lowercase();
    if SSL_MODE_VALUES.contains(&lowered.as_str()) {
        return Ok(());
    }
    Err(DatabaseUrlError::InvalidParameterValue {
        key,
        accepted: &SSL_MODE_VALUES,
    })
}

/// The text shown for a target: the parts that hold no credential, then the parameter names.
fn display_of(url: &Url, parameters: &[&'static str]) -> String {
    let mut shown = format!("{}://", url.scheme());
    if !url.username().is_empty() {
        shown.push_str(url.username());
        shown.push('@');
    }
    shown.push_str(url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        shown.push_str(&format!(":{port}"));
    }
    shown.push_str(url.path());
    if !parameters.is_empty() {
        shown.push_str(&format!(" (parameters: {})", parameters.join(", ")));
    }
    shown
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn target(raw: &str) -> PostgresTarget {
        let url = Url::parse(raw).unwrap();
        PostgresTarget::parse(raw, &url).unwrap()
    }

    #[test]
    fn a_configuration_error_is_told_apart_by_the_part_saslprep_rejects() {
        assert!(matches!(
            target("postgres://mmcp:p%07ss@db/app").explain_configuration_error(),
            DbError::PasswordRejectedBySaslprep
        ));
        assert!(matches!(
            target("postgres://db/app?password=p%07ss").explain_configuration_error(),
            DbError::PasswordRejectedBySaslprep
        ));
        assert!(matches!(
            target("postgres://m%07mcp:pass@db/app").explain_configuration_error(),
            DbError::UsernameRejectedBySaslprep
        ));
        assert!(matches!(
            target("postgres://mmcp:pass@db/app").explain_configuration_error(),
            DbError::ConnectionConfigurationRejected
        ));
    }

    #[test]
    fn the_explained_error_names_no_character_of_the_credential() {
        let error = target("postgres://mmcp:p%07ss@db/app").explain_configuration_error();

        let printed = format!("{error} | {error:?}");
        assert!(
            !printed.contains("p\u{7}ss") && !printed.contains("\\u{7}"),
            "{printed}"
        );
        assert!(std::error::Error::source(&error).is_none());
    }
}
