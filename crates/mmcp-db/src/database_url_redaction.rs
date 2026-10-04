//! Password redaction of a database URL, for log output and error text.

use url::Url;

/// Query keys whose lowercase form contains one of these carry a secret.
const SECRET_QUERY_KEY_MARKERS: [&str; 4] = ["password", "passwd", "secret", "token"];

/// Whether the query parameter `key` carries a secret, whatever its case.
fn is_secret_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_QUERY_KEY_MARKERS
        .iter()
        .any(|marker| key.contains(marker))
}

/// `raw` without the password of its userinfo and without any secret query parameter.
///
/// The other query parameters stay.
/// An unparsable `raw` yields only the text before its first `:`, the scheme, and nothing when it holds no `:`.
/// Nothing more can be shown without risking the password.
#[must_use]
pub fn redact_database_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut url) => {
            // Err only for a URL that cannot carry userinfo, which has no password to remove.
            let _ = url.set_password(None);
            remove_secret_query_parameters(&mut url);
            url.into()
        }
        Err(_) => raw
            .split_once(':')
            .map(|(scheme, _)| scheme.to_owned())
            .unwrap_or_default(),
    }
}

/// Drops every secret parameter from the query of `url`, leaving a query without one untouched.
fn remove_secret_query_parameters(url: &mut Url) {
    if !url.query_pairs().any(|(key, _)| is_secret_query_key(&key)) {
        return;
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_secret_query_key(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_removed_and_the_rest_of_the_url_is_kept() {
        let redacted = redact_database_url("postgres://mmcp:s3cr3t@postgres:5432/mmcp");
        assert!(!redacted.contains("s3cr3t"));
        assert_eq!(redacted, "postgres://mmcp@postgres:5432/mmcp");
    }

    #[test]
    fn percent_encoded_password_is_removed() {
        let redacted = redact_database_url("postgres://mmcp:p%40ss%2Fword@db/app");
        assert!(!redacted.contains("p%40ss"));
        assert!(!redacted.contains("word"));
        assert_eq!(redacted, "postgres://mmcp@db/app");
    }

    #[test]
    fn url_without_a_password_is_unchanged() {
        assert_eq!(redact_database_url("sqlite::memory:"), "sqlite::memory:");
        assert_eq!(
            redact_database_url("sqlite://data/mmcp.db?mode=rwc"),
            "sqlite://data/mmcp.db?mode=rwc"
        );
        assert_eq!(
            redact_database_url("postgres://mmcp@db/app"),
            "postgres://mmcp@db/app"
        );
    }

    #[test]
    fn password_in_the_query_string_is_removed_and_other_parameters_are_kept() {
        let redacted =
            redact_database_url("postgres://db/mmcp?user=mmcp&password=s3cr3t&sslmode=require");
        assert!(!redacted.contains("s3cr3t"));
        assert!(!redacted.contains("password"));
        assert_eq!(redacted, "postgres://db/mmcp?user=mmcp&sslmode=require");
    }

    #[test]
    fn every_secret_looking_query_key_is_removed_whatever_its_case() {
        let redacted = redact_database_url(
            "postgres://mmcp:p1@db/mmcp?PassWord=p2&sslpassword=p3&token=p4&mode=rwc",
        );
        for secret in ["p1", "p2", "p3", "p4"] {
            assert!(!redacted.contains(secret), "{redacted}");
        }
        assert_eq!(redacted, "postgres://mmcp@db/mmcp?mode=rwc");
    }

    #[test]
    fn a_url_whose_only_parameter_is_secret_loses_its_query() {
        assert_eq!(
            redact_database_url("postgres://db/mmcp?password=s3cr3t"),
            "postgres://db/mmcp"
        );
    }

    #[test]
    fn unparsable_text_without_a_colon_shows_nothing() {
        assert_eq!(redact_database_url("s3cr3t-without-a-scheme"), "");
    }

    #[test]
    fn unparsable_url_never_shows_more_than_its_scheme() {
        let redacted = redact_database_url("postgres://mmcp:s3cr3t@[bad-host/app");
        assert!(!redacted.contains("s3cr3t"));
        assert_eq!(redacted, "postgres");
    }
}
