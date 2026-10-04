//! [`DatabaseTarget`], the database a server is configured to use, parsed once from its URL.

use std::fmt;
use std::str::FromStr;

use url::Url;

use super::database_url_error::DatabaseUrlError;
use super::postgres_target::PostgresTarget;
use super::sqlite_target::SqliteTarget;

/// A database URL that passed every refusal rule of mmcp, by backend.
///
/// The rules run here, at parse, on the text as written.
/// sqlx reads the text again at connect, so the rules mirror what sqlx would accept, ignore or log.
/// They refuse the forms that put a credential where the URL grammar does not expect one, and the parameters that sqlx would log with their value.
/// A refusal quotes no part of the text beyond its scheme.
/// Display and Debug show the target without its password.
/// What a dependency does with a text that no rule refuses is outside these guarantees.
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
            "sqlite" => SqliteTarget::parse(raw).map(Self::Sqlite),
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
    use crate::connection::ParameterSet;

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
                    set: ParameterSet::Postgres,
                    position: 2,
                    count: 2,
                },
            ),
            (
                "postgres://mmcp@db/app?PassWord=SECRET_13",
                DatabaseUrlError::ParameterWrongCase {
                    set: ParameterSet::Postgres,
                    position: 1,
                    count: 1,
                    canonical: "password",
                },
            ),
            (
                "postgres://mmcp@db/app?sslpassword=SECRET_14",
                DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Postgres,
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
                DatabaseUrlError::SqliteAtSignInFileName,
            ),
        ];
        for (input, expected) in refused {
            let outcome = input.parse::<DatabaseTarget>();
            assert_eq!(outcome.as_ref().err(), Some(&expected), "{input}");
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn only_a_closed_options_member_belongs_to_the_options_family() {
        for input in [
            "postgres://h/db?options[y]=v",
            "postgres://h/db?options[]=v",
            "postgres://h/db?options[a]b]=v",
        ] {
            assert!(input.parse::<DatabaseTarget>().is_ok(), "{input}");
        }
        for input in [
            "postgres://h/db?options[y=SECRET_O1",
            "postgres://h/db?options[y=SECRET_O2&sslmode=disable",
            "postgres://h/db?options[y]z=SECRET_O3",
            "postgres://h/db?options[=SECRET_O4",
        ] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();

            assert!(
                matches!(
                    error,
                    DatabaseUrlError::UnrecognizedParameter { .. }
                        | DatabaseUrlError::ParameterWithoutValue
                ),
                "{input}: {error}"
            );
            assert!(!chain_text(&error).contains("SECRET_O"), "{input}");
        }
        assert_eq!(
            "postgres://h/db?options[y=v"
                .parse::<DatabaseTarget>()
                .err(),
            Some(DatabaseUrlError::UnrecognizedParameter {
                set: ParameterSet::Postgres,
                position: 1,
                count: 1,
            })
        );
    }

    #[test]
    fn the_remedy_for_a_missing_authority_keeps_the_database_name() {
        let error = "postgres:app?host=/var/run/postgresql"
            .parse::<DatabaseTarget>()
            .unwrap_err();

        assert_eq!(error, DatabaseUrlError::PostgresWithoutAuthority);
        let text = error.to_string();
        assert!(text.contains("postgres:///DB?host="), "{text}");
        // The remedy the text gives is accepted, and the bare prefix it warns about turns the database into a host.
        let remedy = "postgres:///app?host=/var/run/postgresql".parse::<DatabaseTarget>();
        assert_eq!(
            remedy.unwrap().to_string(),
            "postgres:///app (parameters: host)"
        );
        let bare = "postgres://app?host=/var/run/postgresql".parse::<DatabaseTarget>();
        assert_eq!(
            bare.unwrap().to_string(),
            "postgres://app (parameters: host)"
        );
    }

    #[test]
    fn a_dot_segment_cannot_erase_the_at_sign_of_misplaced_password_text() {
        for input in [
            "postgres://u:123/SECRET_1/tail@db/../app",
            "postgres://u:123/SECRET_2/tail@db/%2e%2e/app",
            "postgres://u:123/SECRET_3/tail@db/%2E%2E/app",
            "postgres://u:123/SECRET_4/tail@db/./app",
        ] {
            assert_eq!(
                input.parse::<DatabaseTarget>().unwrap_err(),
                DatabaseUrlError::AtSignOutsideUserinfo,
                "{input}"
            );
            printed(input, "SECRET_").unwrap_err();
        }
    }

    /// SCRAM refuses these credentials, and a server that authenticates by cleartext, MD5, LDAP, RADIUS or PAM takes them.
    /// A refusal at parse would lock that server out, so the connect classifies a SCRAM failure instead.
    #[test]
    fn a_credential_that_scram_would_refuse_is_accepted_at_parse() {
        for input in [
            "postgres://mmcp:p%07SECRET_1@db/app",
            "postgres://db/app?password=p%07SECRET_2",
            "postgres://mmcp:pass%F0%9F%98%80SECRET_3@db/app",
            "postgres://mmcp:abc%D7%A9%D7%9C%D7%95%D7%9D123SECRET_4@db/app",
        ] {
            printed(input, "SECRET_").unwrap_or_else(|text| panic!("{input}: {text}"));
        }
        for input in [
            "postgres://m%07x:pw@db/app",
            "postgres://db/app?user=m%07x",
            "postgres://%D7%A9%D7%9C%D7%95%D7%9D:pw@db/app",
        ] {
            assert!(input.parse::<DatabaseTarget>().is_ok(), "{input}");
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
    fn a_sqlite_query_key_outside_the_accepted_set_is_refused_without_echoing_it() {
        let refused = [
            (
                "sqlite::memory:?password=SECRET_1",
                DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Sqlite,
                    position: 1,
                    count: 1,
                },
            ),
            (
                "sqlite:data/x.db?passwd=SECRET_2",
                DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Sqlite,
                    position: 1,
                    count: 1,
                },
            ),
            (
                "sqlite://data/x.db?mode=rwc&token=SECRET_3",
                DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Sqlite,
                    position: 2,
                    count: 2,
                },
            ),
            (
                "sqlite://data/x.db?SECRET_4",
                DatabaseUrlError::UnrecognizedParameter {
                    set: ParameterSet::Sqlite,
                    position: 1,
                    count: 1,
                },
            ),
            (
                "sqlite://data/x.db?Mode=SECRET_5",
                DatabaseUrlError::ParameterWrongCase {
                    set: ParameterSet::Sqlite,
                    position: 1,
                    count: 1,
                    canonical: "mode",
                },
            ),
        ];
        for (input, expected) in refused {
            assert_eq!(
                input.parse::<DatabaseTarget>().unwrap_err(),
                expected,
                "{input}"
            );
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn a_sqlite_value_outside_the_closed_set_of_its_key_is_refused_without_echoing_it() {
        for input in [
            "sqlite://data/x.db?mode=SECRET_1",
            "sqlite://data/x.db?cache=SECRET_2",
            "sqlite://data/x.db?immutable=SECRET_3",
            "sqlite://data/x.db?mode=",
        ] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();
            assert!(
                matches!(error, DatabaseUrlError::InvalidParameterValue { .. }),
                "{input}"
            );
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn a_sqlite_fragment_is_refused_without_echoing_it() {
        for input in [
            "sqlite://u:#SECRET_1/db",
            "sqlite:data/x.db#SECRET_2",
            "sqlite://data/x.db?mode=rwc#SECRET_3",
            "sqlite://data/x.db#",
        ] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();
            assert_eq!(error, DatabaseUrlError::SqliteFragmentPresent, "{input}");
            assert!(error.to_string().contains("%23"), "{error}");
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn every_accepted_sqlite_key_and_value_stays_accepted() {
        for input in [
            "sqlite://data/x.db?mode=ro&cache=shared&immutable=1&vfs=unix",
            "sqlite://data/x.db?mode=memory&cache=private&immutable=false",
            "sqlite://data/x.db?mode=rw&immutable=true&immutable=0",
        ] {
            assert_eq!(printed(input, "SECRET_").as_deref(), Ok(input));
        }
    }

    #[test]
    fn a_raw_at_sign_in_a_sqlite_url_is_refused_with_the_sqlite_remedy() {
        for input in [
            "sqlite:u:SECRET_1@x.db",
            "sqlite://u:SECRET_2@data/x.db",
            "sqlite://data/x.db?vfs=SECRET_3@y",
            "sqlite://alice@archive/data.db",
        ] {
            let error = input.parse::<DatabaseTarget>().unwrap_err();
            assert_eq!(error, DatabaseUrlError::SqliteAtSignInFileName, "{input}");
            assert!(error.to_string().contains("%40"), "{error}");
            assert!(!error.to_string().contains("dbname"), "{error}");
            printed(input, "SECRET_").unwrap_err();
        }
    }

    #[test]
    fn an_encoded_at_sign_in_a_sqlite_file_name_is_accepted() {
        for input in [
            "sqlite:data/alice%40example.db",
            "sqlite://alice%40archive/data.db",
            "sqlite://data/backup%402024.db?mode=rwc",
        ] {
            assert_eq!(printed(input, "SECRET_").as_deref(), Ok(input));
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
