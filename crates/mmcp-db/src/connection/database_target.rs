//! [`DatabaseTarget`], the database a server is configured to use, parsed once from its URL.

use std::fmt;
use std::str::FromStr;

use url::Url;

use super::database_url_error::DatabaseUrlError;
use super::postgres_target::PostgresTarget;
use super::sqlite_target::SqliteTarget;

/// A database URL that passed every refusal rule, by backend.
///
/// The URL is parsed once, here.
/// A URL with the credential in a place the URL grammar does not expect one is refused, so no part of a password reaches a dependency or a log.
/// Display shows the target without any credential, and Debug shows the same.
#[derive(Debug, Clone)]
pub enum DatabaseTarget {
    Postgres(PostgresTarget),
    Sqlite(SqliteTarget),
}

impl FromStr for DatabaseTarget {
    type Err = DatabaseUrlError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(raw).map_err(|source| DatabaseUrlError::Unparsable { source })?;
        match url.scheme() {
            "postgres" | "postgresql" => PostgresTarget::parse(raw, &url).map(Self::Postgres),
            "sqlite" => SqliteTarget::parse(raw, &url).map(Self::Sqlite),
            other => Err(DatabaseUrlError::UnsupportedScheme {
                scheme: other.to_owned(),
            }),
        }
    }
}

impl fmt::Display for DatabaseTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Postgres(target) => target.fmt(formatter),
            Self::Sqlite(target) => target.fmt(formatter),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::error::Error;

    use super::*;

    /// The text of `error` and of every source behind it, in Display and Debug form.
    fn chain_text(error: &(dyn Error + 'static)) -> String {
        let mut texts = vec![format!("{error} | {error:?}")];
        let mut source = error.source();
        while let Some(cause) = source {
            texts.push(format!("{cause} | {cause:?}"));
            source = cause.source();
        }
        texts.join("\n")
    }

    /// Parses `input`, which holds the sentinel `secret`, and returns the whole printable outcome.
    fn printed(input: &str, secret: &str) -> Result<String, String> {
        match input.parse::<DatabaseTarget>() {
            Ok(target) => {
                let text = format!("{target} | {target:?}");
                assert!(!text.contains(secret), "{input}: {text}");
                Ok(target.to_string())
            }
            Err(error) => {
                let text = chain_text(&error);
                assert!(!text.contains(secret), "{input}: {text}");
                Err(text)
            }
        }
    }

    #[test]
    fn accepted_postgres_forms_display_without_any_credential() {
        let cases = [
            (
                "postgres://mmcp:SECRET_A@db:5432/app",
                "postgres://mmcp@db:5432/app",
            ),
            (
                "postgres://mmcp:SECRET_B%40x@db/app",
                "postgres://mmcp@db/app",
            ),
            (
                "postgres://db/app?password=SECRET_C&sslmode=require",
                "postgres://db/app (parameters: password, sslmode)",
            ),
            (
                "postgres://mmcp:SECRET_D@db/app?password=SECRET_D",
                "postgres://mmcp@db/app (parameters: password)",
            ),
            (
                "postgresql://mmcp:SECRET_E@db/app",
                "postgresql://mmcp@db/app",
            ),
            (
                "postgres:///app?host=/var/run/postgresql&password=SECRET_F",
                "postgres:///app (parameters: host, password)",
            ),
            (
                "postgres://mmcp@db/app?sslkey=SECRET_G&user=SECRET_G",
                "postgres://mmcp@db/app (parameters: sslkey, user)",
            ),
            (
                "postgres://h/db?options=-c%20x%3DSECRET_H&options[y]=SECRET_H",
                "postgres://h/db (parameters: options, options[NAME])",
            ),
            (
                "postgres://h/db?pass%77ord=SECRET_I",
                "postgres://h/db (parameters: password)",
            ),
            (
                "postgres://u:SECRET_J@[::1]:5432/db",
                "postgres://u@[::1]:5432/db",
            ),
            ("postgres:///db", "postgres:///db"),
            (
                "postgres://h/db?sslmode=require&&user=u&",
                "postgres://h/db (parameters: sslmode, user)",
            ),
        ];
        for (input, expected) in cases {
            let secret = "SECRET_";
            assert_eq!(printed(input, secret).as_deref(), Ok(expected), "{input}");
        }
    }

    #[test]
    fn sqlite_forms_display_as_written() {
        for input in [
            "sqlite::memory:",
            "sqlite:data/x.db",
            "sqlite://data/x.db?mode=rwc",
        ] {
            assert_eq!(printed(input, "SECRET_").as_deref(), Ok(input));
        }
    }

    #[test]
    fn every_misplaced_credential_form_is_refused_without_echoing_it() {
        let refused: [(&str, DatabaseUrlError); 17] = [
            (
                "postgres:/mmcp:SECRET_1@db/app",
                DatabaseUrlError::PostgresWithoutAuthority,
            ),
            (
                "postgres:mmcp:SECRET_2@db/app",
                DatabaseUrlError::PostgresWithoutAuthority,
            ),
            (
                "postgres://mmcp:#SECRET_3@db/app",
                DatabaseUrlError::FragmentPresent,
            ),
            (
                "postgres://mmcp:1234#SECRET_4@db/app",
                DatabaseUrlError::FragmentPresent,
            ),
            (
                "postgres://mmcp:/SECRET_5@db/app",
                DatabaseUrlError::AtSignOutsideUserinfo,
            ),
            (
                "postgres://mmcp:123/SECRET_6@db/app",
                DatabaseUrlError::AtSignOutsideUserinfo,
            ),
            (
                "postgres://mmcp:?SECRET_7@db/app",
                DatabaseUrlError::AtSignOutsideUserinfo,
            ),
            (
                "postgres://mmcp:1?k=SECRET_8@db/app",
                DatabaseUrlError::AtSignOutsideUserinfo,
            ),
            (
                "postgres:///user:SECRET_9%40host/db",
                DatabaseUrlError::AtSignOutsideUserinfo,
            ),
            (
                "postgres://mmcp@db/app?SECRET_10",
                DatabaseUrlError::ParameterWithoutValue,
            ),
            (
                "postgres://db/app?password=ab&SECRET_11",
                DatabaseUrlError::ParameterWithoutValue,
            ),
            (
                "postgres://db/app?password=ab&SECRET_12=ef",
                DatabaseUrlError::UnrecognizedParameter {
                    position: 2,
                    count: 2,
                },
            ),
            (
                "postgres://mmcp@db/app?PassWord=SECRET_13",
                DatabaseUrlError::ParameterWrongCase {
                    position: 1,
                    count: 1,
                    canonical: "password",
                },
            ),
            (
                "postgres://mmcp@db/app?sslpassword=SECRET_14",
                DatabaseUrlError::UnrecognizedParameter {
                    position: 1,
                    count: 1,
                },
            ),
            (
                "postgres://db/app?password=ab&sslmode=SECRET_15",
                DatabaseUrlError::InvalidParameterValue {
                    key: "sslmode",
                    accepted: &[
                        "disable",
                        "allow",
                        "prefer",
                        "require",
                        "verify-ca",
                        "verify-full",
                    ],
                },
            ),
            (
                "postgress://mmcp:SECRET_16@db/app",
                DatabaseUrlError::UnsupportedScheme {
                    scheme: "postgress".to_owned(),
                },
            ),
            (
                "sqlite://u:SECRET_17@data/x.db",
                DatabaseUrlError::SqliteUrlWithCredentials,
            ),
        ];
        for (input, expected) in refused {
            let outcome = input.parse::<DatabaseTarget>();
            assert_eq!(outcome.as_ref().err(), Some(&expected), "{input}");
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn unparsable_text_is_refused_with_the_parse_error_as_source() {
        for input in [
            "postgres://mmcp:SECRET_1@[bad-host/app",
            "postgres://mmcp:SECRET_2@/app",
        ] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();
            assert!(
                matches!(error, DatabaseUrlError::Unparsable { .. }),
                "{input}"
            );
            assert!(error.source().is_some());
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn sqlite_misplaced_credentials_are_refused() {
        for input in [
            "sqlite:u:SECRET_1@x.db",
            "sqlite:data/SECRET_2%40x.db",
            "sqlite://data/x.db?k=SECRET_3@y",
        ] {
            assert_eq!(
                input.parse::<DatabaseTarget>().unwrap_err(),
                DatabaseUrlError::AtSignOutsideUserinfo,
                "{input}"
            );
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn urls_that_sqlx_reads_as_written_stay_accepted_where_no_rule_applies() {
        for input in [
            "postgres://h/db",
            "postgres://h:5432/db?application_name=app",
            "postgres://h/db?statement-cache-capacity=50&port=5432",
            "postgres:///db?host=/run/postgresql",
            "postgres://%2Frun%2Fpostgresql/db",
        ] {
            assert!(input.parse::<DatabaseTarget>().is_ok(), "{input}");
        }
    }

    #[test]
    fn a_legitimate_at_sign_outside_the_userinfo_is_refused_with_its_remedy() {
        for input in ["postgres://h/db?user=a@b", "postgres://h/my@db"] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();
            assert_eq!(error, DatabaseUrlError::AtSignOutsideUserinfo, "{input}");
            assert!(error.to_string().contains("%40"), "{error}");
        }
    }
}
