//! Reading a configuration key.

use mmcp_core::config::{ConfigKey, NoticeResolution};

use super::{ConfigEnvironment, ConfigOpError, NoticeSources, ProjectLocation};

/// The value of `key` in every layer, the effective value and its source.
/// Without a project, the local and project layers read as unset.
///
/// # Errors
/// [`ConfigOpError::LoadUser`], [`ConfigOpError::LoadProject`] or [`ConfigOpError::LoadLocal`] when a file cannot be read.
pub fn get_key(
    environment: &ConfigEnvironment<'_>,
    key: ConfigKey,
    location: &ProjectLocation,
) -> Result<NoticeResolution, ConfigOpError> {
    Ok(NoticeSources::load(environment.home, location.root())?.resolve(key, environment.launch))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use mmcp_core::config::{ConfigScope, LaunchValues, NoticeLaunch, NoticeSource, NoticeValue};

    use super::super::config_fixture::{ConfigFixture, already_excluded};
    use super::super::set_key;
    use super::*;

    const KEY: ConfigKey = ConfigKey::NoticeMdProject;

    #[test]
    fn config_get_reports_every_layer_the_effective_value_and_its_source() {
        let mut fixture = ConfigFixture::new();
        fixture.launch = NoticeLaunch {
            md_project: LaunchValues {
                flag: Some(NoticeValue::Off),
                environment: Some(NoticeValue::On),
            },
            ..NoticeLaunch::default()
        };
        let environment = fixture.environment(already_excluded);
        for (scope, value) in [
            (ConfigScope::Local, NoticeValue::On),
            (ConfigScope::Project, NoticeValue::Off),
            (ConfigScope::User, NoticeValue::Off),
        ] {
            set_key(&environment, KEY, value, scope, &fixture.located()).unwrap();
        }

        let resolution = get_key(&environment, KEY, &fixture.located()).unwrap();

        assert_eq!(resolution.effective, NoticeValue::On);
        assert_eq!(resolution.source, NoticeSource::Local);
        assert_eq!(resolution.layers.local, Some(NoticeValue::On));
        assert_eq!(resolution.layers.project, Some(NoticeValue::Off));
        assert_eq!(resolution.layers.flag, Some(NoticeValue::Off));
        assert_eq!(resolution.layers.environment, Some(NoticeValue::On));
        assert_eq!(resolution.layers.user, Some(NoticeValue::Off));
    }

    #[test]
    fn config_get_without_a_project_reports_local_and_project_unset() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(already_excluded);
        let nowhere = fixture.unlocated();
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::User,
            &nowhere,
        )
        .unwrap();

        let resolution = get_key(&environment, KEY, &nowhere).unwrap();

        assert_eq!(resolution.layers.local, None);
        assert_eq!(resolution.layers.project, None);
        assert_eq!(resolution.effective, NoticeValue::Off);
        assert_eq!(resolution.source, NoticeSource::User);
    }

    #[test]
    fn config_get_defaults_to_on_when_nothing_is_set() {
        let fixture = ConfigFixture::new();

        let resolution = get_key(
            &fixture.environment(already_excluded),
            KEY,
            &fixture.located(),
        )
        .unwrap();

        assert_eq!(resolution.effective, NoticeValue::On);
        assert_eq!(resolution.source, NoticeSource::Default);
    }

    #[test]
    fn config_get_resolves_each_key_independently() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(already_excluded);
        let nowhere = fixture.unlocated();
        set_key(
            &environment,
            KEY,
            NoticeValue::Off,
            ConfigScope::User,
            &nowhere,
        )
        .unwrap();

        let other = get_key(&environment, ConfigKey::NoticeMdUser, &nowhere).unwrap();

        assert_eq!(other.effective, NoticeValue::On);
        assert_eq!(other.source, NoticeSource::Default);
    }

    #[test]
    fn config_get_reports_a_broken_project_file_instead_of_resolving_past_it() {
        let fixture = ConfigFixture::new();
        std::fs::write(fixture.project.join(".mmcp.toml"), "not = [valid").unwrap();

        let error = get_key(
            &fixture.environment(already_excluded),
            KEY,
            &fixture.located(),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigOpError::LoadProject(_)), "{error:?}");
    }
}
