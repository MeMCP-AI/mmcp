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
    /// The driver error is dropped, since it may quote a credential or a line of the key file.
    /// The cause is read from the state that decides it: the client certificate and key the URL or the environment give, and the credentials the environment gives.
    pub(super) fn explain_configuration_error(&self) -> DbError {
        self.explain_configuration_error_in(&|name| std::env::var(name).ok())
    }

    /// [`Self::explain_configuration_error`] with the environment as a lookup, so a test sets none.
    ///
    /// The URL's own credentials passed the SASLprep check at parse, so only the environment can still fail it.
    /// It fails after the handshake, and the key checks fail during it, so a failing environment credential beside a client certificate or key leaves the cause unknown.
    fn explain_configuration_error_in(
        &self,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> DbError {
        let Ok(url) = Url::parse(self.exposed_text()) else {
            return DbError::ConnectionConfigurationRejected;
        };
        let has_parameter =
            |keys: [&str; 2]| url.query_pairs().any(|(name, _)| keys.contains(&&*name));
        let has_certificate =
            has_parameter(["sslcert", "ssl-cert"]) || environment("PGSSLCERT").is_some();
        let has_key = has_parameter(["sslkey", "ssl-key"]) || environment("PGSSLKEY").is_some();
        let environment_problem = environment_credential_problem(&url, environment);
        match (environment_problem, has_certificate || has_key) {
            (Some(problem), false) => problem,
            (Some(_), true) => DbError::ConnectionConfigurationRejected,
            (None, _) => match (has_certificate, has_key) {
                (true, false) | (false, true) => DbError::ClientCertificateAndKeyNotPaired,
                (true, true) => DbError::ClientKeyRejected,
                (false, false) => DbError::ConnectionConfigurationRejected,
            },
        }
    }
}

/// The SASLprep failure of a credential that only the environment gives, since the URL gives none of it.
fn environment_credential_problem(
    url: &Url,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Option<DbError> {
    let gives_user =
        !url.username().is_empty() || url.query_pairs().any(|(name, _)| name == "user");
    let gives_password =
        url.password().is_some() || url.query_pairs().any(|(name, _)| name == "password");
    if !gives_user
        && environment("PGUSER")
            .is_some_and(|user| stringprep::saslprep(&format!("n={user}")).is_err())
    {
        return Some(DbError::UsernameRejectedBySaslprep);
    }
    if !gives_password
        && environment("PGPASSWORD")
            .is_some_and(|password| stringprep::saslprep(&password).is_err())
    {
        return Some(DbError::PasswordRejectedBySaslprep);
    }
    None
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::collections::HashMap;

    use super::*;

    fn target(raw: &str) -> PostgresTarget {
        let url = Url::parse(raw).unwrap();
        PostgresTarget::parse(raw, &url).unwrap()
    }

    /// The explanation of a configuration error for `raw`, with the environment `variables`.
    fn explained(raw: &str, variables: &[(&str, &str)]) -> DbError {
        let map: HashMap<&str, &str> = variables.iter().copied().collect();
        target(raw).explain_configuration_error_in(&|name| map.get(name).map(|v| (*v).to_owned()))
    }

    #[test]
    fn a_client_certificate_without_its_key_is_named() {
        for raw in [
            "postgres://db/app?sslmode=require&sslcert=/c.pem",
            "postgres://db/app?ssl-cert=/c.pem",
            "postgres://db/app?sslkey=/k.pem",
            "postgres://db/app?ssl-key=/k.pem",
        ] {
            assert!(
                matches!(
                    explained(raw, &[]),
                    DbError::ClientCertificateAndKeyNotPaired
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn the_environment_can_complete_the_pair() {
        assert!(matches!(
            explained(
                "postgres://db/app?sslcert=/c.pem",
                &[("PGSSLKEY", "/k.pem")]
            ),
            DbError::ClientKeyRejected
        ));
        assert!(matches!(
            explained("postgres://db/app", &[("PGSSLCERT", "/c.pem")]),
            DbError::ClientCertificateAndKeyNotPaired
        ));
    }

    #[test]
    fn a_complete_pair_that_still_fails_is_a_key_failure_without_its_source() {
        for raw in [
            "postgres://db/app?sslcert=/c.pem&sslkey=/k.pem",
            "postgres://db/app?ssl-cert=/c.pem&ssl-key=/k.pem",
        ] {
            let error = explained(raw, &[]);

            assert!(matches!(error, DbError::ClientKeyRejected), "{raw}");
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn an_environment_credential_that_scram_refuses_is_named_without_its_character() {
        let password = explained("postgres://u@db/app", &[("PGPASSWORD", "p\u{7}ss")]);
        let user = explained("postgres://:pw@db/app", &[("PGUSER", "m\u{7}mcp")]);

        assert!(matches!(password, DbError::PasswordRejectedBySaslprep));
        assert!(matches!(user, DbError::UsernameRejectedBySaslprep));
        for error in [password, user] {
            let printed = format!("{error} | {error:?}");
            assert!(
                !printed.contains('\u{7}') && !printed.contains("\\u{7}"),
                "{printed}"
            );
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn a_credential_the_url_gives_hides_the_environment_one() {
        assert!(matches!(
            explained("postgres://u:good@db/app", &[("PGPASSWORD", "p\u{7}ss")]),
            DbError::ConnectionConfigurationRejected
        ));
        assert!(matches!(
            explained(
                "postgres://db/app?password=good",
                &[("PGPASSWORD", "p\u{7}ss")]
            ),
            DbError::ConnectionConfigurationRejected
        ));
    }

    #[test]
    fn an_environment_credential_beside_a_client_key_leaves_the_cause_unknown() {
        assert!(matches!(
            explained(
                "postgres://u@db/app?sslcert=/c.pem&sslkey=/k.pem",
                &[("PGPASSWORD", "p\u{7}ss")]
            ),
            DbError::ConnectionConfigurationRejected
        ));
    }

    #[test]
    fn without_a_known_cause_the_explanation_is_the_generic_variant() {
        assert!(matches!(
            explained("postgres://u:pw@db/app", &[]),
            DbError::ConnectionConfigurationRejected
        ));
    }
}
