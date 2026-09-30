//! User-level configuration loaded from `~/.mmcp/config.toml`.
//!
//! Holds defaults that apply across all projects: sync server,
//! author identity, and default group. Project-level `.mmcp.toml`
//! overrides these when both are set.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ignored_key_path::ignored_key_path;
use super::{
    CLAUDE_MD_TABLE_KEY, ClaudeMdSetOutcome, ClaudeMdSuggestion, ClaudeMdTable, ConfigDiagnostic,
    ConfigError, SyncConfig, UserProjectConfig,
};
use crate::id::ProjectUuid;

/// Key of the per-project table in `~/.mmcp/config.toml`.
const PROJECTS_TABLE_KEY: &str = "projects";

/// User-level configuration. All fields are optional; a missing
/// config file is equivalent to an empty struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// Default sync remotes, inherited by every project unless a
    /// project sets its own `project_remote_only = true`. Always
    /// present (`SyncConfig::default()` when unset), the same shape
    /// as `ProjectConfig.sync`, so the sync surface is uniform
    /// between user and project level.
    #[serde(default, skip_serializing_if = "SyncConfig::is_empty")]
    pub sync: SyncConfig,

    /// Author identity for commits.
    pub author: Option<AuthorConfig>,

    /// Default values for CLI flags.
    pub defaults: Option<DefaultsConfig>,

    /// Tunable numeric limits an operator may override per-install.
    pub limits: Option<LimitsConfig>,

    /// CLAUDE.md suggestion setting for every project.
    #[serde(skip_serializing_if = "ClaudeMdTable::is_empty")]
    pub claude_md: ClaudeMdTable,

    /// The user's own settings per project, keyed by the project UUID.
    /// An entry that carries nothing is removed.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, UserProjectConfig>,
}

/// Author identity configuration.
///
/// The `git_fallback` field is a tri-state:
/// - `Some(true)`: read `git config --global user.name/email` when
///   `name`/`email` are not set here.
/// - `Some(false)`: never read git config. Use mmcp hardcoded
///   fallback. User explicitly opted out.
/// - `None` (default): not decided. Diagnose reports a warning.
///   Resolution falls back to constants without reading git.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorConfig {
    /// Override commit author name.
    pub name: Option<String>,

    /// Override commit author email.
    pub email: Option<String>,

    /// Whether to fall back to `git config user.name/email`.
    /// Must be explicitly set to `true` to enable. Never reads
    /// git config silently.
    pub git_fallback: Option<bool>,
}

/// Default values for CLI commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default group UUID or slug for `--group` flags.
    pub group: Option<String>,
}

/// Per-install overrides for numeric limits that otherwise fall back
/// to a compiled-in constant. Each field names the constant it
/// overrides in its own doc comment so the override and its fallback
/// stay discoverable from either side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Overrides `mmcp_store::memory::DEFAULT_MAX_AUTO_SLUG_LENGTH`,
    /// the cap applied to a slug auto-derived from a title
    /// (`add_issue` / `add_feature` / milestone creation / file
    /// import default-slug path) when no per-call override and no
    /// `MMCP_MAX_AUTO_SLUG_LENGTH` environment variable are set.
    pub max_auto_slug_length: Option<usize>,

    /// Overrides `mmcp_auth::password::MIN_PASSWORD_LENGTH`, in
    /// bytes, the minimum accepted account password length. Beaten
    /// by a CLI `--min-password-length` override or the
    /// `MMCP_MIN_PASSWORD_LENGTH` environment variable; wins over
    /// the compiled-in default when neither of those is set.
    pub min_password_length: Option<usize>,

    /// Overrides `mmcp_auth::password::MAX_PASSWORD_LENGTH`, in
    /// bytes, the maximum accepted account password length. Same
    /// precedence cascade as [`LimitsConfig::min_password_length`].
    pub max_password_length: Option<usize>,

    /// Overrides `mmcp_auth::backend::MAX_HANDLE_LENGTH`, in bytes, the maximum accepted account handle length.
    /// Same precedence cascade as [`LimitsConfig::min_password_length`].
    pub max_handle_length: Option<usize>,
}

impl UserConfig {
    /// Parse from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Parse`] on malformed TOML, or
    /// [`ConfigError::DuplicateRemoteName`] /
    /// [`ConfigError::MultipleDefaultRemotes`] when `sync.remotes`
    /// fails [`SyncConfig::validate`]'s single-file, single-level
    /// checks. Widened from `toml::de::Error` to [`ConfigError`] to
    /// carry these new semantic-validation failures; mirrors
    /// `ProjectConfig::from_toml`.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Self::from_toml_with_diagnostics(text).map(|(cfg, _diagnostics)| cfg)
    }

    /// Parse from a TOML string and report what the loader tolerated: every ignored key, every invalid `claude_md` table, every `projects` entry not keyed by a UUID.
    ///
    /// # Errors
    /// The errors of [`UserConfig::from_toml`].
    pub fn from_toml_with_diagnostics(
        text: &str,
    ) -> Result<(Self, Vec<ConfigDiagnostic>), ConfigError> {
        let deserializer = toml::de::Deserializer::parse(text).map_err(ConfigError::from)?;
        let mut ignored_paths = Vec::new();
        let cfg: Self = serde_ignored::deserialize(deserializer, |path| {
            ignored_paths.push(ignored_key_path(&path));
        })
        .map_err(ConfigError::from)?;
        cfg.sync.validate()?;
        let diagnostics = ignored_paths
            .into_iter()
            .map(|key_path| ConfigDiagnostic::UnknownKey { key_path })
            .chain(cfg.tolerated_mistakes())
            .collect();
        Ok((cfg, diagnostics))
    }

    /// Diagnostics of the tables this config holds: invalid or unknown-keyed `claude_md` tables and misspelt `projects` keys.
    fn tolerated_mistakes(&self) -> Vec<ConfigDiagnostic> {
        let global = self.claude_md.diagnostics(CLAUDE_MD_TABLE_KEY);
        let per_project = self.projects.iter().flat_map(|(key, entry)| {
            let keyed_by_uuid = Uuid::parse_str(key).is_ok();
            let key_diagnostic =
                (!keyed_by_uuid).then(|| ConfigDiagnostic::ProjectKeyNotUuid { key: key.clone() });
            let table_path = format!("{PROJECTS_TABLE_KEY}.{key}.{CLAUDE_MD_TABLE_KEY}");
            key_diagnostic
                .into_iter()
                .chain(entry.claude_md.diagnostics(&table_path))
        });
        global.into_iter().chain(per_project).collect()
    }

    /// The user's `claude_md` table for a project, `None` when the user has no entry for it.
    /// Keys are matched as UUIDs, so any spelling of the UUID finds its entry.
    #[must_use]
    pub fn project_claude_md(&self, project: ProjectUuid) -> Option<&ClaudeMdTable> {
        self.projects
            .iter()
            .find(|(key, _)| Self::key_is_project(key, project))
            .map(|(_, entry)| &entry.claude_md)
    }

    /// Set the user's CLAUDE.md suggestion for every project, or remove it with `None`.
    pub fn set_claude_md_suggestion(
        &mut self,
        mode: Option<ClaudeMdSuggestion>,
    ) -> ClaudeMdSetOutcome {
        self.claude_md.set(mode)
    }

    /// Set the user's CLAUDE.md suggestion for one project, or remove it with `None`.
    /// An entry left empty is removed.
    pub fn set_project_claude_md_suggestion(
        &mut self,
        project: ProjectUuid,
        mode: Option<ClaudeMdSuggestion>,
    ) -> ClaudeMdSetOutcome {
        let key = self
            .projects
            .keys()
            .find(|key| Self::key_is_project(key, project))
            .cloned()
            .unwrap_or_else(|| project.to_string());
        let entry = self.projects.entry(key.clone()).or_default();
        let outcome = entry.claude_md.set(mode);
        if entry.is_empty() {
            self.projects.remove(&key);
        }
        outcome
    }

    fn key_is_project(key: &str, project: ProjectUuid) -> bool {
        Uuid::parse_str(key).is_ok_and(|parsed| parsed == *project.as_uuid())
    }

    /// Render to a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::from)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn empty_string_parses_to_defaults() {
        let cfg: UserConfig = toml::from_str("").unwrap();
        assert!(cfg.sync.is_empty());
        assert!(cfg.author.is_none());
        assert!(cfg.defaults.is_none());
        assert!(cfg.limits.is_none());
    }

    #[test]
    fn full_config_round_trips() {
        let text = r#"
[sync]
server_url = "https://mmcp.example.com"

[author]
name = "Alice"
email = "alice@example.com"
git_fallback = true

[defaults]
group = "my-group"

[limits]
max_auto_slug_length = 80
min_password_length = 10
max_password_length = 128
max_handle_length = 32
"#;
        let cfg = UserConfig::from_toml(text).unwrap();
        assert_eq!(cfg.author.as_ref().unwrap().name.as_deref(), Some("Alice"));
        assert_eq!(cfg.author.as_ref().unwrap().git_fallback, Some(true));
        assert_eq!(
            cfg.sync.server_url.as_deref(),
            Some("https://mmcp.example.com")
        );
        assert_eq!(
            cfg.defaults.as_ref().unwrap().group.as_deref(),
            Some("my-group")
        );
        assert_eq!(cfg.limits.as_ref().unwrap().max_auto_slug_length, Some(80));
        assert_eq!(cfg.limits.as_ref().unwrap().min_password_length, Some(10));
        assert_eq!(cfg.limits.as_ref().unwrap().max_password_length, Some(128));
        assert_eq!(cfg.limits.as_ref().unwrap().max_handle_length, Some(32));
    }

    #[test]
    fn limits_password_length_fields_are_none_when_omitted() {
        let text = "[limits]\nmax_auto_slug_length = 80\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.limits.as_ref().unwrap().min_password_length.is_none());
        assert!(cfg.limits.as_ref().unwrap().max_password_length.is_none());
        assert!(cfg.limits.as_ref().unwrap().max_handle_length.is_none());
    }

    #[test]
    fn limits_section_omitted_when_absent() {
        let text = "[author]\nname = \"Bob\"\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.limits.is_none());
    }

    #[test]
    fn git_fallback_none_when_omitted() {
        let text = "[author]\nname = \"Bob\"\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.author.as_ref().unwrap().git_fallback.is_none());
    }

    const PROJECT_UUID: &str = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91";

    fn project_uuid() -> ProjectUuid {
        ProjectUuid::from_uuid(Uuid::parse_str(PROJECT_UUID).unwrap())
    }

    #[test]
    fn user_config_without_new_keys_renders_unchanged() {
        let cfg = UserConfig::from_toml("[author]\nname = \"Bob\"\n").unwrap();
        let rendered = cfg.to_toml().unwrap();
        assert!(!rendered.contains("claude_md"), "{rendered}");
        assert!(!rendered.contains("projects"), "{rendered}");
    }

    #[test]
    fn user_config_global_and_per_project_entries_round_trip_keyed_by_uuid() {
        let mut cfg = UserConfig::default();
        cfg.set_claude_md_suggestion(Some(ClaudeMdSuggestion::Decline));
        cfg.set_project_claude_md_suggestion(project_uuid(), Some(ClaudeMdSuggestion::Suggest));

        let rendered = cfg.to_toml().unwrap();
        assert!(
            rendered.contains(&format!("[projects.{PROJECT_UUID}.claude_md]")),
            "{rendered}"
        );
        let reparsed = UserConfig::from_toml(&rendered).unwrap();

        assert_eq!(
            reparsed.claude_md.suggestion(),
            Ok(Some(ClaudeMdSuggestion::Decline))
        );
        assert_eq!(
            reparsed
                .project_claude_md(project_uuid())
                .unwrap()
                .suggestion(),
            Ok(Some(ClaudeMdSuggestion::Suggest))
        );
    }

    #[test]
    fn a_project_entry_is_found_under_any_spelling_of_its_uuid() {
        let text = format!(
            "[projects.{}.claude_md]\nproject_file_suggestion = \"decline\"\n",
            PROJECT_UUID.to_uppercase()
        );
        let cfg = UserConfig::from_toml(&text).unwrap();
        assert_eq!(
            cfg.project_claude_md(project_uuid()).unwrap().suggestion(),
            Ok(Some(ClaudeMdSuggestion::Decline))
        );
    }

    #[test]
    fn setting_inherit_at_user_project_removes_the_emptied_project_entry() {
        let mut cfg = UserConfig::default();
        cfg.set_project_claude_md_suggestion(project_uuid(), Some(ClaudeMdSuggestion::Decline));
        assert!(cfg.project_claude_md(project_uuid()).is_some());

        let outcome = cfg.set_project_claude_md_suggestion(project_uuid(), None);

        assert!(outcome.changed);
        assert!(cfg.projects.is_empty());
        assert!(cfg.project_claude_md(project_uuid()).is_none());
    }

    #[test]
    fn setters_report_changed_false_when_the_value_is_already_there() {
        let mut cfg = UserConfig::default();
        assert!(
            cfg.set_claude_md_suggestion(Some(ClaudeMdSuggestion::Decline))
                .changed
        );
        assert!(
            !cfg.set_claude_md_suggestion(Some(ClaudeMdSuggestion::Decline))
                .changed
        );
        assert!(
            !cfg.set_project_claude_md_suggestion(project_uuid(), None)
                .changed
        );
        assert!(
            cfg.projects.is_empty(),
            "inherit on a missing entry adds none"
        );
    }

    #[test]
    fn user_config_with_an_invalid_claude_md_value_still_loads_sync_author_and_limits() {
        let text = r#"
[sync]
server_url = "https://mmcp.example.com"

[author]
name = "Alice"

[limits]
max_auto_slug_length = 80

[claude_md]
project_file_suggestion = "maybe"
"#;
        let (cfg, diagnostics) = UserConfig::from_toml_with_diagnostics(text)
            .expect("an invalid claude_md value never fails the user config");

        assert_eq!(
            cfg.sync.server_url.as_deref(),
            Some("https://mmcp.example.com")
        );
        assert_eq!(cfg.author.as_ref().unwrap().name.as_deref(), Some("Alice"));
        assert_eq!(cfg.limits.as_ref().unwrap().max_auto_slug_length, Some(80));
        assert_eq!(
            diagnostics,
            vec![ConfigDiagnostic::ClaudeMdSettingInvalid {
                table_path: "claude_md".to_string(),
                error: crate::config::ClaudeMdSettingError::InvalidValue {
                    value: "\"maybe\"".to_string()
                },
            }]
        );
    }

    #[test]
    fn user_config_unknown_key_anywhere_is_a_typed_warning_with_its_key_path() {
        let text = "[author]\nname = \"Bob\"\nnick = \"b\"\n\n[rogue]\nx = 1\n";
        let (_cfg, diagnostics) = UserConfig::from_toml_with_diagnostics(text).unwrap();
        assert_eq!(
            diagnostics,
            vec![
                ConfigDiagnostic::UnknownKey {
                    key_path: "author.nick".to_string()
                },
                ConfigDiagnostic::UnknownKey {
                    key_path: "rogue".to_string()
                },
            ]
        );
    }

    #[test]
    fn misspelt_claude_md_table_name_is_a_typed_warning_naming_it() {
        let text = format!(
            "[claude-md]\nproject_file_suggestion = \"decline\"\n\n[projects.{PROJECT_UUID}.claud_md]\nproject_file_suggestion = \"decline\"\n"
        );
        let (cfg, diagnostics) = UserConfig::from_toml_with_diagnostics(&text).unwrap();
        assert_eq!(
            diagnostics,
            vec![
                ConfigDiagnostic::UnknownKey {
                    key_path: "claude-md".to_string()
                },
                ConfigDiagnostic::UnknownKey {
                    key_path: format!("projects.{PROJECT_UUID}.claud_md")
                },
            ]
        );
        assert!(cfg.claude_md.is_empty());
    }

    #[test]
    fn a_project_entry_not_keyed_by_a_uuid_is_a_typed_warning() {
        let text = "[projects.my-project.claude_md]\nproject_file_suggestion = \"decline\"\n";
        let (_cfg, diagnostics) = UserConfig::from_toml_with_diagnostics(text).unwrap();
        assert_eq!(
            diagnostics,
            vec![ConfigDiagnostic::ProjectKeyNotUuid {
                key: "my-project".to_string()
            }]
        );
    }

    #[test]
    fn unknown_key_inside_a_claude_md_table_is_reported_with_the_table_path() {
        let text = format!(
            "[projects.{PROJECT_UUID}.claude_md]\nproject_file_suggestion = \"decline\"\nfuture = true\n"
        );
        let (_cfg, diagnostics) = UserConfig::from_toml_with_diagnostics(&text).unwrap();
        assert_eq!(
            diagnostics,
            vec![ConfigDiagnostic::UnknownKey {
                key_path: format!("projects.{PROJECT_UUID}.claude_md.future")
            }]
        );
    }

    #[test]
    fn invalid_claude_md_table_raw_text_survives_an_unrelated_save() {
        let text = format!(
            "[author]\nname = \"Bob\"\n\n[claude_md]\nproject_file_suggestion = \"maybe\"\n\n[projects.{PROJECT_UUID}.claude_md]\nproject_file_suggestion = 7\n"
        );
        let mut cfg = UserConfig::from_toml(&text).unwrap();
        cfg.author.as_mut().unwrap().name = Some("Alice".to_string());

        let rendered = cfg.to_toml().unwrap();
        let (reparsed, diagnostics) = UserConfig::from_toml_with_diagnostics(&rendered).unwrap();

        assert_eq!(cfg.claude_md, reparsed.claude_md);
        assert_eq!(cfg.projects, reparsed.projects);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(rendered.contains("maybe"), "{rendered}");
        assert!(rendered.contains("= 7"), "{rendered}");
    }

    /// Same uniform `SyncConfig` surface as `ProjectConfig`:
    /// `remotes` parses, round-trips, and exposes the same helper
    /// methods at user level.
    #[test]
    fn remotes_round_trip_at_user_level() {
        let text = r#"
[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://mmcp.example.com"
default = true
"#;
        let cfg = UserConfig::from_toml(text).unwrap();
        assert_eq!(cfg.sync.remotes.len(), 1);
        assert_eq!(cfg.sync.remotes[0].name(), "primary");
        assert!(cfg.sync.remotes[0].is_default());

        let rendered = cfg.to_toml().unwrap();
        let reparsed = UserConfig::from_toml(&rendered).unwrap();
        assert_eq!(cfg.sync, reparsed.sync);
    }

    /// The same single-file, single-level duplicate-name check
    /// applies at user level as at project level.
    #[test]
    fn duplicate_remote_names_are_rejected_at_user_level() {
        let text = r#"
[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://a.example.com"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://b.example.com"
"#;
        let err = UserConfig::from_toml(text).expect_err("duplicate name must be rejected");
        assert!(matches!(
            err,
            ConfigError::DuplicateRemoteName { name } if name == "primary"
        ));
    }
}
