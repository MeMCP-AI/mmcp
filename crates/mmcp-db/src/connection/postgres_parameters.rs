//! The query parameters of a Postgres URL that sqlx 0.9 recognizes, which mmcp mirrors.
//!
//! A parameter outside this set is refused, because sqlx would log its value at warn and ignore it.
//! The set belongs to the sqlx adapter and changes with the driver.

/// Every query key sqlx recognizes, apart from the `options[NAME]` family.
pub(super) const ACCEPTED_KEYS: [&str; 18] = [
    "sslmode",
    "ssl-mode",
    "sslrootcert",
    "ssl-root-cert",
    "ssl-ca",
    "sslcert",
    "ssl-cert",
    "sslkey",
    "ssl-key",
    "statement-cache-capacity",
    "host",
    "hostaddr",
    "port",
    "dbname",
    "user",
    "password",
    "application_name",
    "options",
];

/// A query key starting with this prefix is a startup option of the `options[NAME]` family.
pub(super) const OPTIONS_FAMILY_PREFIX: &str = "options[";

/// How the `options[NAME]` family is shown, since its members are operator text.
pub(super) const OPTIONS_FAMILY_DISPLAY: &str = "options[NAME]";

/// The query keys that set the SSL mode.
pub(super) const SSL_MODE_KEYS: [&str; 2] = ["sslmode", "ssl-mode"];

/// The values sqlx accepts for the SSL mode, compared without regard to case.
pub(super) const SSL_MODE_VALUES: [&str; 6] = [
    "disable",
    "allow",
    "prefer",
    "require",
    "verify-ca",
    "verify-full",
];

/// The accepted keys and the `options[NAME]` family, as a comma-separated list for an error text.
pub(super) fn accepted_keys_text() -> String {
    let mut keys: Vec<&str> = ACCEPTED_KEYS.to_vec();
    keys.push(OPTIONS_FAMILY_DISPLAY);
    keys.join(", ")
}

/// The constant that a recognized `key` is shown as, or `None` when sqlx does not recognize it.
pub(super) fn accepted_constant(key: &str) -> Option<&'static str> {
    if key.starts_with(OPTIONS_FAMILY_PREFIX) {
        return Some(OPTIONS_FAMILY_DISPLAY);
    }
    ACCEPTED_KEYS
        .iter()
        .find(|accepted| **accepted == key)
        .copied()
}

/// The accepted key that `key` equals when the case is ignored, for a key written with the wrong case.
pub(super) fn canonical_by_case(key: &str) -> Option<&'static str> {
    ACCEPTED_KEYS
        .iter()
        .find(|accepted| accepted.eq_ignore_ascii_case(key))
        .copied()
}
