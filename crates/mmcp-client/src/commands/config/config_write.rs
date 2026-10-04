//! Setting and unsetting a configuration key at a scope.

use std::path::{Path, PathBuf};

use mmcp_core::config::{ConfigKey, ConfigScope, NoticeConfig, NoticeValue};
use mmcp_git::checkout::Exclusion;
use mmcp_store::config::{
    config_path_for, load as load_project, load_local, local_config_path, save as save_project,
    save_local,
};

use super::{ConfigEnvironment, ConfigOpError, ConfigWriteOutcome, NoticeSources, ProjectLocation};

/// Set `key` to `value` at `scope`.
/// Idempotent: a value already stored writes nothing and reports `changed = false`.
///
/// # Errors
/// [`ConfigOpError::ProjectRootRequired`] for the `project` and `local` scopes without a project, otherwise the load, save or exclusion failure of the scope's file.
pub fn set_key(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    value: NoticeValue,
    scope: ConfigScope,
    location: &ProjectLocation,
) -> Result<ConfigWriteOutcome, ConfigOpError> {
    write_key(environment, key, Some(value), scope, location)
}

/// Remove `key` at `scope`, so the next layer decides.
/// Idempotent: a key that is not stored writes nothing and reports `changed = false`.
///
/// # Errors
/// The errors of [`set_key`].
pub fn unset_key(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    scope: ConfigScope,
    location: &ProjectLocation,
) -> Result<ConfigWriteOutcome, ConfigOpError> {
    write_key(environment, key, None, scope, location)
}

/// Apply `value` (`None` removes the key) to `table`, `true` when it changed.
fn apply(table: &mut NoticeConfig, key: ConfigKey, value: Option<NoticeValue>) -> bool {
    match value {
        Some(value) => table.set(key, value),
        None => table.unset(key),
    }
}

/// The file a write landed in, and the global excludes file it appended to, if any.
struct Written {
    file: PathBuf,
    changed: bool,
    excluded_in: Option<PathBuf>,
}

fn write_key(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    value: Option<NoticeValue>,
    scope: ConfigScope,
    location: &ProjectLocation,
) -> Result<ConfigWriteOutcome, ConfigOpError> {
    let written = match scope {
        ConfigScope::User => write_user(environment, key, value)?,
        ConfigScope::Project => write_project(key, value, location.require_root(scope)?)?,
        ConfigScope::Local => write_local(environment, key, value, location.require_root(scope)?)?,
    };
    let resolution =
        NoticeSources::load(environment.home, location.root())?.resolve(key, environment.launch);
    Ok(ConfigWriteOutcome {
        key,
        scope,
        value,
        changed: written.changed,
        file: written.file,
        excluded_in: written.excluded_in,
        resolution,
    })
}

fn write_user(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    value: Option<NoticeValue>,
) -> Result<Written, ConfigOpError> {
    let mut config = environment
        .home
        .load_user_config()
        .map_err(ConfigOpError::LoadUser)?;
    let changed = apply(&mut config.notice, key, value);
    if changed {
        environment
            .home
            .save_user_config(&config)
            .map_err(ConfigOpError::SaveUser)?;
    }
    Ok(Written {
        file: environment.home.user_config_path(),
        changed,
        excluded_in: None,
    })
}

fn write_project(
    key: ConfigKey,
    value: Option<NoticeValue>,
    root: &Path,
) -> Result<Written, ConfigOpError> {
    let mut config = load_project(root).map_err(ConfigOpError::LoadProject)?;
    let changed = apply(&mut config.notice, key, value);
    if changed {
        save_project(root, &config).map_err(ConfigOpError::SaveProject)?;
    }
    Ok(Written {
        file: config_path_for(root),
        changed,
        excluded_in: None,
    })
}

fn write_local(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    value: Option<NoticeValue>,
    root: &Path,
) -> Result<Written, ConfigOpError> {
    let file = local_config_path(root).map_err(ConfigOpError::LoadLocal)?;
    let mut config = load_local(root).map_err(ConfigOpError::LoadLocal)?;
    let changed = apply(&mut config.notice, key, value);
    let mut excluded_in = None;
    if changed {
        // Excluded before the first byte is written, so the file is never left committable.
        match (environment.exclude_local)(&file).map_err(ConfigOpError::ExcludeLocal)? {
            Exclusion::Appended { file: excludes } => excluded_in = Some(excludes),
            Exclusion::Tracked => return Err(ConfigOpError::LocalTracked { file }),
            Exclusion::NotInRepository | Exclusion::AlreadyExcluded => {}
        }
        save_local(root, &config).map_err(ConfigOpError::SaveLocal)?;
    }
    Ok(Written {
        file,
        changed,
        excluded_in,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use mmcp_core::config::NoticeSource;

    use super::super::config_fixture::{
        ConfigFixture, PROJECT_UUID, SCRATCH_EXCLUDES_FILE, already_excluded, already_tracked,
        appends_to_scratch_excludes, fails_to_exclude, outside_a_repository,
    };
    use super::*;

    const KEY: ConfigKey = ConfigKey::NoticeMdProject;

    #[test]
    fn config_set_local_writes_the_local_file_and_leaves_mmcp_toml_byte_identical() {
        let fixture = ConfigFixture::new();
        let before = fixture.project_toml();

        let outcome = set_key(
            &fixture.environment(appends_to_scratch_excludes),
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.file, fixture.project.join(".mmcp.local.toml"));
        assert_eq!(fixture.project_toml(), before);
        assert!(
            std::fs::read_to_string(&outcome.file)
                .unwrap()
                .contains("project = \"off\"")
        );
        assert_eq!(outcome.resolution.effective, NoticeValue::Off);
        assert_eq!(outcome.resolution.source, NoticeSource::Local);
    }

    #[test]
    fn config_set_project_writes_mmcp_toml_and_it_reloads() {
        let fixture = ConfigFixture::new();

        let outcome = set_key(
            &fixture.environment(already_excluded),
            KEY,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.file, fixture.project.join(".mmcp.toml"));
        let reloaded = load_project(&fixture.project).unwrap();
        assert_eq!(reloaded.notice.get(KEY), Some(NoticeValue::Off));
        assert_eq!(reloaded.project_uuid.to_string(), PROJECT_UUID);
        assert!(!fixture.project.join(".mmcp.local.toml").exists());
        assert_eq!(outcome.resolution.source, NoticeSource::Project);
    }

    #[test]
    fn config_set_user_writes_the_served_home_config_without_a_project() {
        let fixture = ConfigFixture::new();

        let outcome = set_key(
            &fixture.environment(already_excluded),
            ConfigKey::NoticeMdUser,
            NoticeValue::Off,
            ConfigScope::User,
            &fixture.unlocated(),
        )
        .unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.file, fixture.home.user_config_path());
        let reloaded = fixture.home.load_user_config().unwrap();
        assert_eq!(
            reloaded.notice.get(ConfigKey::NoticeMdUser),
            Some(NoticeValue::Off)
        );
        assert_eq!(outcome.resolution.source, NoticeSource::User);
    }

    #[test]
    fn config_set_repeats_as_unchanged_and_writes_nothing() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(appends_to_scratch_excludes);
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        let repeat = set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        assert!(!repeat.changed);
        assert_eq!(
            repeat.excluded_in, None,
            "nothing written, nothing excluded"
        );
    }

    #[test]
    fn config_unset_removes_the_key_and_repeats_with_changed_false() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(appends_to_scratch_excludes);
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();

        let removed =
            unset_key(&environment, KEY, ConfigScope::Project, &fixture.located()).unwrap();
        let repeat =
            unset_key(&environment, KEY, ConfigScope::Project, &fixture.located()).unwrap();

        assert!(removed.changed);
        assert_eq!(removed.value, None);
        assert!(!repeat.changed);
        assert!(!fixture.project_toml().contains("notice"));
        assert_eq!(removed.resolution.source, NoticeSource::Default);
    }

    #[test]
    fn config_unset_at_local_empties_the_file_without_touching_other_scopes() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(already_excluded);
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();
        set_key(
            &environment,
            KEY,
            NoticeValue::On,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        let outcome = unset_key(&environment, KEY, ConfigScope::Local, &fixture.located()).unwrap();

        assert_eq!(std::fs::read_to_string(&outcome.file).unwrap(), "");
        assert_eq!(outcome.resolution.effective, NoticeValue::Off);
        assert_eq!(outcome.resolution.source, NoticeSource::Project);
    }

    #[test]
    fn config_set_project_or_local_without_a_project_requires_a_project_root() {
        let fixture = ConfigFixture::new();
        for scope in [ConfigScope::Project, ConfigScope::Local] {
            let error = set_key(
                &fixture.environment(already_excluded),
                KEY,
                NoticeValue::Off,
                scope,
                &fixture.unlocated(),
            )
            .unwrap_err();
            assert!(
                matches!(&error, ConfigOpError::ProjectRootRequired { scope: required, searched_from }
                    if *required == scope && *searched_from == fixture.unsearched()),
                "{error:?}"
            );
        }
        assert!(!fixture.home.user_config_path().exists());
    }

    #[test]
    fn config_unset_project_or_local_without_a_project_requires_a_project_root() {
        let fixture = ConfigFixture::new();
        let error = unset_key(
            &fixture.environment(already_excluded),
            KEY,
            ConfigScope::Local,
            &fixture.unlocated(),
        )
        .unwrap_err();
        assert!(matches!(error, ConfigOpError::ProjectRootRequired { .. }));
    }

    #[test]
    fn first_local_write_reports_the_excludes_file_it_appended_to() {
        let fixture = ConfigFixture::new();

        let outcome = set_key(
            &fixture.environment(appends_to_scratch_excludes),
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        assert_eq!(
            outcome.excluded_in,
            Some(PathBuf::from(SCRATCH_EXCLUDES_FILE))
        );
    }

    #[test]
    fn local_write_where_the_file_is_already_ignored_or_outside_git_reports_no_excludes_file() {
        for exclusion in [already_excluded, outside_a_repository] {
            let fixture = ConfigFixture::new();

            let outcome = set_key(
                &fixture.environment(exclusion),
                KEY,
                NoticeValue::Off,
                ConfigScope::Local,
                &fixture.located(),
            )
            .unwrap();

            assert_eq!(outcome.excluded_in, None);
            assert!(outcome.file.exists());
        }
    }

    #[test]
    fn a_failing_exclusion_refuses_the_local_write_and_leaves_no_file() {
        let fixture = ConfigFixture::new();

        let error = set_key(
            &fixture.environment(fails_to_exclude),
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigOpError::ExcludeLocal(_)), "{error:?}");
        assert!(!fixture.project.join(".mmcp.local.toml").exists());
    }

    #[test]
    fn project_and_user_writes_never_invoke_the_exclusion() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(fails_to_exclude);

        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::User,
            &fixture.unlocated(),
        )
        .unwrap();
    }

    #[test]
    fn a_malformed_file_of_the_scope_is_a_typed_load_error_naming_its_step() {
        let fixture = ConfigFixture::new();
        std::fs::write(
            fixture.project.join(".mmcp.local.toml"),
            "[notice.md]\nprojet = \"off\"\n",
        )
        .unwrap();

        let error = set_key(
            &fixture.environment(already_excluded),
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigOpError::LoadLocal(_)), "{error:?}");
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn a_tracked_local_file_is_refused_before_anything_is_written() {
        let fixture = ConfigFixture::new();

        let error = set_key(
            &fixture.environment(already_tracked),
            KEY,
            NoticeValue::Off,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap_err();

        assert!(
            matches!(&error, ConfigOpError::LocalTracked { file }
                if *file == fixture.project.join(".mmcp.local.toml")),
            "{error:?}"
        );
        assert!(!fixture.project.join(".mmcp.local.toml").exists());
    }

    #[test]
    fn a_tracked_local_file_does_not_block_a_write_that_changes_nothing() {
        let fixture = ConfigFixture::new();

        let outcome = unset_key(
            &fixture.environment(already_tracked),
            KEY,
            ConfigScope::Local,
            &fixture.located(),
        )
        .unwrap();

        assert!(!outcome.changed);
    }

    #[test]
    fn setting_one_key_leaves_the_other_key_of_the_same_file_untouched() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(already_excluded);
        set_key(
            &environment,
            ConfigKey::NoticeMdUser,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();

        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::Project,
            &fixture.located(),
        )
        .unwrap();

        let reloaded = load_project(&fixture.project).unwrap();
        assert_eq!(
            reloaded.notice.get(ConfigKey::NoticeMdUser),
            Some(NoticeValue::Off)
        );
        assert_eq!(reloaded.notice.get(KEY), Some(NoticeValue::Off));
    }
}
