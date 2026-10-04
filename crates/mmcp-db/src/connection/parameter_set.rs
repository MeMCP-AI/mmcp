//! [`ParameterSet`], the driver whose query parameters a refusal speaks of.

use std::fmt;

use super::postgres_parameters;
use super::sqlite_parameters;

/// The driver whose set of accepted query parameters applies to a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSet {
    Postgres,
    Sqlite,
}

impl ParameterSet {
    /// The accepted parameters of the driver, as a comma-separated list for an error text.
    pub(super) fn accepted_text(self) -> String {
        match self {
            Self::Postgres => postgres_parameters::accepted_keys_text(),
            Self::Sqlite => sqlite_parameters::ACCEPTED_KEYS.join(", "),
        }
    }
}

impl fmt::Display for ParameterSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        })
    }
}
