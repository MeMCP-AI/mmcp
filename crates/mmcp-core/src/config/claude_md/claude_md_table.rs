//! [`ClaudeMdTable`], the lenient `claude_md` table shared by every configuration level.

use serde::{Deserialize, Serialize};
use toml::Value;

use super::{ClaudeMdSetOutcome, ClaudeMdSettingError, ClaudeMdSuggestion};
use crate::config::ConfigDiagnostic;

/// Key of the `claude_md` table in every configuration file.
pub const CLAUDE_MD_TABLE_KEY: &str = "claude_md";

/// Key of the suggestion setting inside a `claude_md` table.
pub const PROJECT_FILE_SUGGESTION_KEY: &str = "project_file_suggestion";

/// A `claude_md` table, held as the TOML value the file carries.
/// Holding the raw value keeps an unknown key and an invalid table across a load and save, so a re-render never erases what another version wrote.
/// Reading the setting is a separate phase ([`ClaudeMdTable::suggestion`]), so a mistake inside the table never fails the enclosing file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaudeMdTable(Value);

impl Default for ClaudeMdTable {
    fn default() -> Self {
        Self(Value::Table(toml::Table::new()))
    }
}

impl ClaudeMdTable {
    /// Whether the table carries nothing, so the enclosing file renders without it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(&self.0, Value::Table(table) if table.is_empty())
    }

    /// The setting this table holds, `None` when it holds none.
    ///
    /// # Errors
    /// [`ClaudeMdSettingError`] when the table is not a table or its value is not accepted.
    pub fn suggestion(&self) -> Result<Option<ClaudeMdSuggestion>, ClaudeMdSettingError> {
        let Value::Table(table) = &self.0 else {
            return Err(ClaudeMdSettingError::NotATable {
                found: self.0.type_str(),
            });
        };
        let Some(value) = table.get(PROJECT_FILE_SUGGESTION_KEY) else {
            return Ok(None);
        };
        value
            .clone()
            .try_into::<ClaudeMdSuggestion>()
            .map(Some)
            .map_err(|_| ClaudeMdSettingError::InvalidValue {
                value: value.to_string(),
            })
    }

    /// Keys of the table other than the setting, in key order.
    #[must_use]
    pub fn unknown_keys(&self) -> Vec<&str> {
        match &self.0 {
            Value::Table(table) => table
                .keys()
                .map(String::as_str)
                .filter(|key| *key != PROJECT_FILE_SUGGESTION_KEY)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Diagnostics of this table, whose own key path in its file is `table_path`.
    #[must_use]
    pub fn diagnostics(&self, table_path: &str) -> Vec<ConfigDiagnostic> {
        let invalid =
            self.suggestion()
                .err()
                .map(|error| ConfigDiagnostic::ClaudeMdSettingInvalid {
                    table_path: table_path.to_string(),
                    error,
                });
        let unknown = self
            .unknown_keys()
            .into_iter()
            .map(|key| ConfigDiagnostic::UnknownKey {
                key_path: format!("{table_path}.{key}"),
            });
        invalid.into_iter().chain(unknown).collect()
    }

    /// Set the setting, or remove it with `None`.
    /// An invalid table or value is replaced, and the outcome says so.
    pub fn set(&mut self, mode: Option<ClaudeMdSuggestion>) -> ClaudeMdSetOutcome {
        let replaced_invalid = self.suggestion().is_err();
        let before = self.0.clone();
        if !matches!(self.0, Value::Table(_)) {
            self.0 = Value::Table(toml::Table::new());
        }
        if let Value::Table(table) = &mut self.0 {
            match mode {
                Some(suggestion) => {
                    table.insert(
                        PROJECT_FILE_SUGGESTION_KEY.to_string(),
                        Value::String(suggestion.as_str().to_string()),
                    );
                }
                None => {
                    table.remove(PROJECT_FILE_SUGGESTION_KEY);
                }
            }
        }
        ClaudeMdSetOutcome {
            changed: before != self.0,
            replaced_invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn table(source: &str) -> ClaudeMdTable {
        ClaudeMdTable(toml::from_str::<Value>(source).expect("parse table"))
    }

    #[test]
    fn an_empty_table_holds_no_setting_and_is_empty() {
        let table = ClaudeMdTable::default();
        assert!(table.is_empty());
        assert_eq!(table.suggestion(), Ok(None));
    }

    #[test]
    fn a_valid_value_is_read() {
        assert_eq!(
            table("project_file_suggestion = \"decline\"").suggestion(),
            Ok(Some(ClaudeMdSuggestion::Decline))
        );
        assert_eq!(
            table("project_file_suggestion = \"suggest\"").suggestion(),
            Ok(Some(ClaudeMdSuggestion::Suggest))
        );
    }

    #[test]
    fn an_invalid_value_is_a_typed_error_carrying_the_raw_value() {
        assert_eq!(
            table("project_file_suggestion = \"maybe\"").suggestion(),
            Err(ClaudeMdSettingError::InvalidValue {
                value: "\"maybe\"".to_string()
            })
        );
        assert_eq!(
            table("project_file_suggestion = 3").suggestion(),
            Err(ClaudeMdSettingError::InvalidValue {
                value: "3".to_string()
            })
        );
    }

    #[test]
    fn a_value_that_is_not_a_table_is_a_typed_error() {
        let not_a_table = ClaudeMdTable(Value::String("decline".to_string()));
        assert_eq!(
            not_a_table.suggestion(),
            Err(ClaudeMdSettingError::NotATable { found: "string" })
        );
    }

    #[test]
    fn an_unknown_key_is_reported_with_its_key_path_and_the_known_key_still_applies() {
        let table = table("project_file_suggestion = \"decline\"\nproject_file_sugestion = \"x\"");
        assert_eq!(table.suggestion(), Ok(Some(ClaudeMdSuggestion::Decline)));
        assert_eq!(
            table.diagnostics("claude_md"),
            vec![ConfigDiagnostic::UnknownKey {
                key_path: "claude_md.project_file_sugestion".to_string()
            }]
        );
    }

    #[test]
    fn an_invalid_value_is_a_diagnostic_naming_the_table_and_the_value() {
        let table = table("project_file_suggestion = \"maybe\"");
        assert_eq!(
            table.diagnostics("projects.abc.claude_md"),
            vec![ConfigDiagnostic::ClaudeMdSettingInvalid {
                table_path: "projects.abc.claude_md".to_string(),
                error: ClaudeMdSettingError::InvalidValue {
                    value: "\"maybe\"".to_string()
                },
            }]
        );
    }

    #[test]
    fn an_unknown_key_survives_a_set_and_a_render() {
        let mut table = table("future_key = true");
        let outcome = table.set(Some(ClaudeMdSuggestion::Decline));
        assert!(outcome.changed);
        assert!(!outcome.replaced_invalid);
        assert_eq!(table.unknown_keys(), vec!["future_key"]);
        assert_eq!(table.suggestion(), Ok(Some(ClaudeMdSuggestion::Decline)));
    }

    #[test]
    fn setting_the_held_value_reports_no_change() {
        let mut table = table("project_file_suggestion = \"decline\"");
        let outcome = table.set(Some(ClaudeMdSuggestion::Decline));
        assert!(!outcome.changed);
        assert!(!outcome.replaced_invalid);
    }

    #[test]
    fn inherit_removes_the_value_and_reports_no_change_on_repeat() {
        let mut table = table("project_file_suggestion = \"decline\"");
        assert!(table.set(None).changed);
        assert!(table.is_empty());
        assert!(!table.set(None).changed);
    }

    #[test]
    fn a_set_replaces_an_invalid_value_and_says_so() {
        let mut table = table("project_file_suggestion = \"maybe\"");
        let outcome = table.set(Some(ClaudeMdSuggestion::Suggest));
        assert!(outcome.changed);
        assert!(outcome.replaced_invalid);
        assert_eq!(table.suggestion(), Ok(Some(ClaudeMdSuggestion::Suggest)));
    }

    #[test]
    fn a_set_replaces_a_value_that_is_not_a_table_and_says_so() {
        let mut not_a_table = ClaudeMdTable(Value::Boolean(true));
        let outcome = not_a_table.set(Some(ClaudeMdSuggestion::Decline));
        assert!(outcome.changed);
        assert!(outcome.replaced_invalid);
        assert_eq!(
            not_a_table.suggestion(),
            Ok(Some(ClaudeMdSuggestion::Decline))
        );
    }
}
