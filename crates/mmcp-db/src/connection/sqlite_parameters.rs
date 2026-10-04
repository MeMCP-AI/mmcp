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
