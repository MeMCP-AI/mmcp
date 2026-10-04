//! [`PostgresTarget`], a Postgres URL that passed every refusal rule.

use std::fmt;
use std::str::FromStr;

use percent_encoding::percent_decode_str;
use sea_orm::sqlx;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgSslMode};
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

    /// The value-free error for a configuration error that sqlx raised while connecting with `options`.
    ///
    /// The driver error is dropped, since it may quote a credential or a line of the key file.
    /// The cause is read from the state that decides it.
    /// The SSL mode and the user name are the ones sqlx connected with, from `options`.
    /// The client certificate and key, and the password, come from the URL and the environment, since sqlx exposes no getter for them.
    pub(super) fn explain_configuration_error(&self, options: &PgConnectOptions) -> DbError {
        self.explain_configuration_error_in(options, &|name| std::env::var(name).ok())
    }

    /// [`Self::explain_configuration_error`] with the environment as a lookup, so a test sets none.
    ///
    /// SCRAM authentication applies SASLprep to the user name first and to the password later, so a failing user name decides among the SASLprep causes.
    /// Under the SSL modes `disable` and `allow` sqlx never reads a certificate or a key, so no TLS cause exists.
    /// Under the modes `require`, `verify-ca` and `verify-full` a server that declines TLS ends the connect with a TLS error, so a configuration error means the handshake ran.
    /// The handshake comes before authentication, so a lone certificate or key is then the cause, whatever the credentials.
    /// Otherwise a TLS cause is a candidate only: a server that declines TLS under `prefer` skips the checks, and a complete pair can fail after a handshake that succeeded.
    /// A candidate is named only when every SASLprep alternative is excluded: the user name passes, and a password that the URL or `PGPASSWORD` gives passes.
    /// A password that a pgpass file supplies is not known here, so without a given password a candidate leaves the cause unknown.
    fn explain_configuration_error_in(
        &self,
        options: &PgConnectOptions,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> DbError {
        let Ok(url) = Url::parse(self.exposed_text()) else {
            return DbError::ConnectionConfigurationRejected;
        };
        let authentication_cause =
            username_cause(options).or_else(|| password_cause(&url, environment));
        match tls_cause(&url, options.get_ssl_mode(), environment) {
            Some(TlsCause {
                error,
                certain: true,
            }) => error,
            Some(TlsCause { error, .. })
                if authentication_cause.is_none()
                    && given_password(&url, environment).is_some() =>
            {
                error
            }
            Some(_) => DbError::ConnectionConfigurationRejected,
            None => authentication_cause.unwrap_or(DbError::ConnectionConfigurationRejected),
        }
    }
}

/// A TLS error that the client certificate and key the URL or the environment give would raise.
struct TlsCause {
    error: DbError,
    /// Whether the handshake must have raised it, for any credentials and any server answer.
    certain: bool,
}

/// The TLS cause of the client certificate and key the URL or the environment give.
///
/// sqlx returns the socket before it reads either file under the SSL modes `disable` and `allow`.
fn tls_cause(
    url: &Url,
    ssl_mode: PgSslMode,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Option<TlsCause> {
    if matches!(ssl_mode, PgSslMode::Disable | PgSslMode::Allow) {
        return None;
    }
    let tls_is_required = !matches!(ssl_mode, PgSslMode::Prefer);
    let has_parameter = |keys: [&str; 2]| url.query_pairs().any(|(name, _)| keys.contains(&&*name));
    let has_certificate =
        has_parameter(["sslcert", "ssl-cert"]) || environment("PGSSLCERT").is_some();
    let has_key = has_parameter(["sslkey", "ssl-key"]) || environment("PGSSLKEY").is_some();
    match (has_certificate, has_key) {
        (true, false) | (false, true) => Some(TlsCause {
            error: DbError::ClientCertificateAndKeyNotPaired,
            certain: tls_is_required,
        }),
        (true, true) => Some(TlsCause {
            error: DbError::ClientKeyRejected,
            certain: false,
        }),
        (false, false) => None,
    }
}

/// The SASLprep failure of the user name that sqlx connected with, wherever it came from.
///
/// The URL, `PGUSER` and the operating system user name all end in the options, and SCRAM prepares the user name together with its `n=` prefix.
fn username_cause(options: &PgConnectOptions) -> Option<DbError> {
    stringprep::saslprep(&format!("n={}", options.get_username()))
        .is_err()
        .then_some(DbError::UsernameRejectedBySaslprep)
}

/// The password that the URL or `PGPASSWORD` gives, in sqlx's precedence.
///
/// A given password keeps sqlx from reading a pgpass file, so `None` leaves the password unknown.
fn given_password(url: &Url, environment: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    effective_value(url, url.password().map(decoded), "password")
        .or_else(|| environment("PGPASSWORD"))
}

/// The SASLprep failure of the password that the URL or the environment gives.
fn password_cause(url: &Url, environment: &dyn Fn(&str) -> Option<String>) -> Option<DbError> {
    given_password(url, environment)
        .is_some_and(|password| stringprep::saslprep(&password).is_err())
        .then_some(DbError::PasswordRejectedBySaslprep)
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

    /// The explanation of a configuration error for `raw` connected with `options`, with the environment `variables`.
    fn explained_with(
        raw: &str,
        options: &PgConnectOptions,
        variables: &[(&str, &str)],
    ) -> DbError {
        let map: HashMap<&str, &str> = variables.iter().copied().collect();
        target(raw).explain_configuration_error_in(options, &|name| {
            map.get(name).map(|value| (*value).to_owned())
        })
    }

    /// The options that sqlx builds for `raw`, with the host's own `PGUSER`, `PGSSLMODE` and operating system user name left out.
    ///
    /// A user name that the URL does not give is `PGUSER` among the `variables`, else `mmcp`.
    /// An SSL mode that the URL does not give is sqlx's default, `prefer`.
    fn options_for(raw: &str, variables: &[(&str, &str)]) -> PgConnectOptions {
        let url = Url::parse(raw).unwrap();
        let gives = |keys: &[&str]| url.query_pairs().any(|(name, _)| keys.contains(&&*name));
        let mut options = PgConnectOptions::from_str(raw).unwrap();
        if url.username().is_empty() && !gives(&["user"]) {
            let user = variables
                .iter()
                .find_map(|(name, value)| (*name == "PGUSER").then_some(*value));
            options = options.username(user.unwrap_or("mmcp"));
        }
        if !gives(&["sslmode", "ssl-mode"]) {
            options = options.ssl_mode(PgSslMode::Prefer);
        }
        options
    }

    /// The explanation of a configuration error for `raw`, with the environment `variables`.
    fn explained(raw: &str, variables: &[(&str, &str)]) -> DbError {
        explained_with(raw, &options_for(raw, variables), variables)
    }

    /// A password that the URL gives suppresses pgpass, and it passes SASLprep, so every SASLprep alternative is excluded.
    #[test]
    fn a_client_certificate_without_its_key_is_named_where_the_password_is_given() {
        for raw in [
            "postgres://u:pw@db/app?sslmode=require&sslcert=/c.pem",
            "postgres://u:pw@db/app?ssl-cert=/c.pem",
            "postgres://u:pw@db/app?sslkey=/k.pem",
            "postgres://u:pw@db/app?ssl-key=/k.pem",
            "postgres://u@db/app?sslkey=/k.pem&password=pw",
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
    fn the_environment_can_complete_the_pair_and_give_the_password() {
        assert!(matches!(
            explained(
                "postgres://u:pw@db/app?sslcert=/c.pem",
                &[("PGSSLKEY", "/k.pem")]
            ),
            DbError::ClientKeyRejected
        ));
        assert!(matches!(
            explained(
                "postgres://u@db/app",
                &[("PGSSLCERT", "/c.pem"), ("PGPASSWORD", "pw")]
            ),
            DbError::ClientCertificateAndKeyNotPaired
        ));
    }

    #[test]
    fn a_complete_pair_that_still_fails_is_a_key_failure_without_its_source() {
        for raw in [
            "postgres://u:pw@db/app?sslcert=/c.pem&sslkey=/k.pem",
            "postgres://u:pw@db/app?ssl-cert=/c.pem&ssl-key=/k.pem",
            "postgres://u:pw@db/app?sslmode=require&sslcert=/c.pem&sslkey=/k.pem",
        ] {
            let error = explained(raw, &[]);

            assert!(matches!(error, DbError::ClientKeyRejected), "{raw}");
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    /// Where TLS is required, a server that declines it ends the connect with a TLS error, so a configuration error means the handshake ran.
    /// The handshake checks the pair before authentication starts, so the credentials cannot be the cause.
    #[test]
    fn a_lone_certificate_or_key_is_certain_where_tls_is_required() {
        for mode in ["require", "verify-ca", "verify-full"] {
            for query in ["sslcert=/c.pem", "sslkey=/k.pem"] {
                for credentials in ["", "u:pw@", "m%07x:p%07y@"] {
                    let raw = format!("postgres://{credentials}db/app?sslmode={mode}&{query}");

                    assert!(
                        matches!(
                            explained(&raw, &[]),
                            DbError::ClientCertificateAndKeyNotPaired
                        ),
                        "{raw}"
                    );
                }
            }
        }
    }

    /// A pgpass file may supply the password, and sqlx exposes neither the file nor the password.
    /// A server that declines TLS under `prefer` skips the checks, and a complete pair can fail after a handshake that succeeded.
    #[test]
    fn a_tls_candidate_beside_an_opaque_password_leaves_the_cause_unknown() {
        for raw in [
            "postgres://u@db/app?sslcert=/c.pem",
            "postgres://u@db/app?sslkey=/k.pem",
            "postgres://u@db/app?sslcert=/c.pem&sslkey=/k.pem",
            "postgres://u@db/app?sslmode=require&sslcert=/c.pem&sslkey=/k.pem",
            "postgres://u@db/app?sslmode=verify-full&sslcert=/c.pem&sslkey=/k.pem",
        ] {
            assert!(
                matches!(
                    explained(raw, &[]),
                    DbError::ConnectionConfigurationRejected
                ),
                "{raw}"
            );
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

    fn same_variant(left: &DbError, right: &DbError) -> bool {
        std::mem::discriminant(left) == std::mem::discriminant(right)
    }

    #[test]
    fn a_url_credential_that_scram_refuses_is_named_without_its_character() {
        for (raw, expected) in [
            (
                "postgres://u:p%07ss@db/app",
                DbError::PasswordRejectedBySaslprep,
            ),
            (
                "postgres://db/app?password=p%F0%9F%98%80ss",
                DbError::PasswordRejectedBySaslprep,
            ),
            (
                "postgres://u:abc%D7%A9%D7%9C%D7%95%D7%9D123@db/app",
                DbError::PasswordRejectedBySaslprep,
            ),
            (
                "postgres://m%07x:pw@db/app",
                DbError::UsernameRejectedBySaslprep,
            ),
            (
                "postgres://db/app?user=m%07x",
                DbError::UsernameRejectedBySaslprep,
            ),
            (
                "postgres://%D7%A9%D7%9C%D7%95%D7%9D:pw@db/app",
                DbError::UsernameRejectedBySaslprep,
            ),
        ] {
            let error = explained(raw, &[]);

            assert!(same_variant(&error, &expected), "{raw}: {error:?}");
            let printed = format!("{error} | {error:?}");
            assert!(
                !printed.contains('\u{7}')
                    && !printed.contains('\u{1F600}')
                    && !printed.contains("\\u{7}"),
                "{printed}"
            );
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn the_url_credential_that_decides_follows_the_precedence_sqlx_applies() {
        // The query overrides the userinfo, and the last occurrence of a parameter wins.
        for raw in [
            "postgres://u:p%07x@db/app?password=good",
            "postgres://m%07x:pw@db/app?user=good",
            "postgres://u@db/app?password=p%07x&password=good",
            "postgres://u@db/app?user=m%07x&user=good",
        ] {
            assert!(
                matches!(
                    explained(raw, &[]),
                    DbError::ConnectionConfigurationRejected
                ),
                "{raw}"
            );
        }
        for (raw, expected) in [
            (
                "postgres://u:good@db/app?password=p%07x",
                DbError::PasswordRejectedBySaslprep,
            ),
            (
                "postgres://u@db/app?password=good&password=p%07x",
                DbError::PasswordRejectedBySaslprep,
            ),
            (
                "postgres://good:pw@db/app?user=m%07x",
                DbError::UsernameRejectedBySaslprep,
            ),
            (
                "postgres://u@db/app?user=good&user=m%07x",
                DbError::UsernameRejectedBySaslprep,
            ),
        ] {
            let error = explained(raw, &[]);

            assert!(same_variant(&error, &expected), "{raw}: {error:?}");
        }
    }

    #[test]
    fn a_failing_user_name_is_named_before_a_failing_password() {
        assert!(matches!(
            explained("postgres://m%07x:p%07y@db/app", &[]),
            DbError::UsernameRejectedBySaslprep
        ));
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

    /// Under these modes sqlx returns the socket before it reads a certificate or a key.
    #[test]
    fn no_tls_cause_is_named_where_sslmode_means_tls_never_runs() {
        for mode in ["disable", "allow"] {
            for query in [
                format!("sslmode={mode}&sslcert=/c.pem"),
                format!("ssl-mode={mode}&sslkey=/k.pem"),
                format!("sslmode={mode}&sslcert=/c.pem&sslkey=/k.pem"),
            ] {
                let raw = format!("postgres://u:pw@db/app?{query}");

                assert!(
                    matches!(
                        explained(&raw, &[]),
                        DbError::ConnectionConfigurationRejected
                    ),
                    "{raw}"
                );
            }
        }
    }

    #[test]
    fn a_credential_that_scram_refuses_is_named_where_tls_never_runs() {
        assert!(matches!(
            explained(
                "postgres://u@db/app?sslmode=disable&sslcert=/c.pem&sslkey=/k.pem",
                &[("PGPASSWORD", "p\u{7}ss")]
            ),
            DbError::PasswordRejectedBySaslprep
        ));
        assert!(matches!(
            explained(
                "postgres://m%07x:pw@db/app?sslmode=disable&sslcert=/c.pem",
                &[]
            ),
            DbError::UsernameRejectedBySaslprep
        ));
    }

    #[test]
    fn a_tls_cause_is_still_named_under_every_mode_that_negotiates_tls() {
        for mode in ["prefer", "require", "verify-ca", "verify-full"] {
            let unpaired = format!("postgres://u:pw@db/app?sslmode={mode}&sslcert=/c.pem");
            let paired = format!("{unpaired}&sslkey=/k.pem");

            assert!(
                matches!(
                    explained(&unpaired, &[]),
                    DbError::ClientCertificateAndKeyNotPaired
                ),
                "{unpaired}"
            );
            assert!(
                matches!(explained(&paired, &[]), DbError::ClientKeyRejected),
                "{paired}"
            );
        }
    }

    /// The user name that sqlx connected with can come from `PGUSER` or from the operating system, which no URL text shows.
    #[test]
    fn a_user_name_that_scram_refuses_is_read_from_the_options_wherever_it_came_from() {
        let raw = "postgres://db/app?sslmode=disable";
        let options = options_for(raw, &[]).username("m\u{7}x");

        let error = explained_with(raw, &options, &[]);

        assert!(matches!(error, DbError::UsernameRejectedBySaslprep));
        let printed = format!("{error} | {error:?}");
        assert!(!printed.contains('\u{7}') && !printed.contains("\\u{7}"));
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
