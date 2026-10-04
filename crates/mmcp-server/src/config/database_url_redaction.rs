//! Password redaction of a database URL, for log output.

use url::Url;

/// `raw` without the password of its userinfo.
///
/// An unparsable `raw` yields only the text before its first `:` (the scheme),
/// since nothing more can be shown without risking the password.
pub(super) fn redact_database_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut url) => {
            // Err only for a URL that cannot carry userinfo, which has no password to remove.
            let _ = url.set_password(None);
            url.into()
        }
        Err(_) => raw.split(':').next().unwrap_or_default().to_owned(),
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
    fn unparsable_url_never_shows_more_than_its_scheme() {
        let redacted = redact_database_url("postgres://mmcp:s3cr3t@[bad-host/app");
        assert!(!redacted.contains("s3cr3t"));
        assert_eq!(redacted, "postgres");
    }
}
