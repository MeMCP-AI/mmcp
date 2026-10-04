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
use super::parameter_set::ParameterSet;
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
        if has_at_sign_outside_userinfo(raw) {
            return Err(DatabaseUrlError::AtSignOutsideUserinfo);
        }
        if url.query().is_some_and(has_valueless_entry) {
            return Err(DatabaseUrlError::ParameterWithoutValue);
        }
        let parameters = checked_parameters(url)?;
        check_saslprep(url)?;
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
    /// The driver error is dropped, since it may quote a credential.
    pub(super) fn explain_configuration_error(&self) -> DbError {
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

/// Checks every query parameter and returns the constants they are shown as, in order and without repeats.
fn checked_parameters(url: &Url) -> Result<Vec<&'static str>, DatabaseUrlError> {
    let count = url.query_pairs().count();
    let mut shown: Vec<&'static str> = Vec::new();
    for (index, (key, value)) in url.query_pairs().enumerate() {
        let position = index + 1;
        let Some(constant) = accepted_constant(&key) else {
            return Err(match canonical_by_case(&key) {
                Some(canonical) => DatabaseUrlError::ParameterWrongCase {
                    set: ParameterSet::Postgres,
                    position,
                    count,
                    canonical,
                },
                None => DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Postgres,
                    position,
                    count,
                },
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

/// The value that the URL gives for the user name or the password, in sqlx's precedence.
///
/// sqlx reads the userinfo first, then every query parameter in order, so the last `key` parameter wins.
fn effective_value(url: &Url, userinfo: Option<String>, key: &str) -> Option<String> {
    url.query_pairs()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .last()
        .or(userinfo)
}

/// Refuses a user name or a password that SCRAM authentication would refuse, so no connection raises its error.
///
/// SCRAM prepares the user name together with its `n=` prefix, and the password alone.
/// A credential that the environment or a pgpass file supplies is not known here.
fn check_saslprep(url: &Url) -> Result<(), DatabaseUrlError> {
    let user = effective_value(
        url,
        Some(url.username())
            .filter(|user| !user.is_empty())
            .map(decoded),
        "user",
    );
    if user.is_some_and(|user| stringprep::saslprep(&format!("n={user}")).is_err()) {
        return Err(DatabaseUrlError::UsernameRejectedBySaslprep);
    }
    let password = effective_value(url, url.password().map(decoded), "password");
    if password.is_some_and(|password| stringprep::saslprep(&password).is_err()) {
        return Err(DatabaseUrlError::PasswordRejectedBySaslprep);
    }
    Ok(())
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
