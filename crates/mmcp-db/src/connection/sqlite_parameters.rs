//! The query parameters of a SQLite URL that sqlx-sqlite 0.9 recognizes, which mmcp mirrors.
//!
//! A parameter outside this set makes sqlx refuse the URL at connect, after the startup line has shown it.
//! The set belongs to the sqlx adapter and changes with the driver.

/// Every query key sqlx-sqlite recognizes.
pub(super) const ACCEPTED_KEYS: [&str; 4] = ["mode", "cache", "immutable", "vfs"];

/// The value sqlx-sqlite takes for the `mode` key, compared exactly.
pub(super) const MODE_VALUES: [&str; 4] = ["ro", "rw", "rwc", "memory"];

/// The values sqlx-sqlite takes for the `cache` key, compared exactly.
pub(super) const CACHE_VALUES: [&str; 2] = ["private", "shared"];

/// The values sqlx-sqlite takes for the `immutable` key, compared exactly.
pub(super) const IMMUTABLE_VALUES: [&str; 4] = ["true", "1", "false", "0"];

/// The closed set of values of `key`, or `None` for a key that takes free text.
pub(super) fn closed_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "mode" => Some(&MODE_VALUES),
        "cache" => Some(&CACHE_VALUES),
        "immutable" => Some(&IMMUTABLE_VALUES),
        _ => None,
    }
}

/// The accepted key that `key` equals when the case is ignored, for a key written with the wrong case.
pub(super) fn canonical_by_case(key: &str) -> Option<&'static str> {
    ACCEPTED_KEYS
        .iter()
        .find(|accepted| accepted.eq_ignore_ascii_case(key))
        .copied()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sea_orm::sqlx::sqlite::SqliteConnectOptions;

    use super::*;

    /// Whether sqlx-sqlite takes `key=value` as the query of a URL.
    fn sqlx_accepts(key: &str, value: &str) -> bool {
        SqliteConnectOptions::from_str(&format!("sqlite:x.db?{key}={value}")).is_ok()
    }

    /// A value of a key that takes free text.
    const FREE_TEXT_VALUE: &str = "any";

    /// The values that sqlx-sqlite 0.9.0 documents for `mode`, `cache` and `immutable`, written out here and not read from the mirror.
    const DRIVER_VALUES: [&str; 10] = [
        "ro", "rw", "rwc", "memory", "private", "shared", "true", "false", "1", "0",
    ];

    /// The query parameters that SQLite itself interprets, a superset of what sqlx-sqlite recognizes.
    /// They are the "core URI query parameters" of the `sqlite3_open_v2` documentation in the bundled `sqlite3.c`, plus `modeof`, which the unix VFS of the same file reads.
    /// They are written out here and not read from the mirror.
    const SQLITE_URI_PARAMETERS: [&str; 7] = [
        "vfs",
        "mode",
        "cache",
        "psow",
        "nolock",
        "immutable",
        "modeof",
    ];

    /// Values that sqlx-sqlite could widen to, and that the mirror must keep refusing while sqlx does.
    const NEAR_MISSES: [&str; 6] = ["RO", "Private", "TRUE", "yes", "2", ""];

    #[test]
    fn sqlx_sqlite_accepts_every_mirrored_key_with_every_mirrored_value() {
        for key in ACCEPTED_KEYS {
            let values = closed_values(key).unwrap_or(&[FREE_TEXT_VALUE]);
            for value in values {
                assert!(sqlx_accepts(key, value), "{key}={value}");
            }
        }
    }

    /// Whether sqlx-sqlite takes `key` with some value of the probe set.
    fn sqlx_recognizes(key: &str) -> bool {
        DRIVER_VALUES
            .iter()
            .chain([&FREE_TEXT_VALUE])
            .any(|value| sqlx_accepts(key, value))
    }

    /// A key that a later sqlx-sqlite starts to recognize shows here as a key the mirror lacks.
    #[test]
    fn the_mirror_holds_exactly_the_sqlite_uri_parameters_that_sqlx_sqlite_recognizes() {
        for key in SQLITE_URI_PARAMETERS {
            assert_eq!(sqlx_recognizes(key), ACCEPTED_KEYS.contains(&key), "{key}");
        }
    }

    #[test]
    fn sqlx_sqlite_refuses_a_key_the_mirror_does_not_hold() {
        for key in ["password", "token", "Mode", "cached", "mode "] {
            assert!(!ACCEPTED_KEYS.contains(&key), "{key}");
            assert!(!sqlx_accepts(key, "x"), "{key}");
        }
    }

    /// The closed value sets equal the ones sqlx-sqlite takes, as far as a probe of its documented values and the near misses shows.
    #[test]
    fn the_closed_value_sets_agree_with_sqlx_sqlite_on_every_probe() {
        let probes: Vec<&str> = DRIVER_VALUES.into_iter().chain(NEAR_MISSES).collect();
        for key in ACCEPTED_KEYS {
            let Some(values) = closed_values(key) else {
                continue;
            };
            for probe in &probes {
                assert_eq!(
                    sqlx_accepts(key, probe),
                    values.contains(probe),
                    "{key}={probe}"
                );
            }
        }
    }
}
