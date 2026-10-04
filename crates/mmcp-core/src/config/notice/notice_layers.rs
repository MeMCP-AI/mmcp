//! [`NoticeLayers`], the per-layer values one key resolves from.

use super::{NoticeLaunch, NoticeResolution, NoticeSource, NoticeValue};
use crate::config::{ConfigKey, LocalConfig, ProjectConfig, UserConfig};

/// Value of every layer for one key, `None` where a layer sets nothing.
/// Precedence, highest first: `local`, `project`, `flag`, `environment`, `user`, then the default of the key.
/// The launch layers rank below both per-project layers, so a recorded refusal is never undone by a launch value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoticeLayers {
    /// `.mmcp.local.toml`.
    pub local: Option<NoticeValue>,
    /// `.mmcp.toml`.
    pub project: Option<NoticeValue>,
    /// The launch flag of the serving process.
    pub flag: Option<NoticeValue>,
    /// The launch environment variable of the serving process.
    pub environment: Option<NoticeValue>,
    /// `~/.mmcp/config.toml`.
    pub user: Option<NoticeValue>,
}

impl NoticeLayers {
    /// The layers of `key` read from the loaded configs and the launch values.
    /// `local` and `project` are `None` when no project root resolved.
    #[must_use]
    pub fn collect(
        key: ConfigKey,
        local: Option<&LocalConfig>,
        project: Option<&ProjectConfig>,
        user: &UserConfig,
        launch: &NoticeLaunch,
    ) -> Self {
        let launched = launch.for_key(key);
        Self {
            local: local.and_then(|config| config.notice.get(key)),
            project: project.and_then(|config| config.notice.get(key)),
            flag: launched.flag,
            environment: launched.environment,
            user: user.notice.get(key),
        }
    }

    /// Every layer with its value, in precedence order, highest first.
    #[must_use]
    pub const fn entries(self) -> [(NoticeSource, Option<NoticeValue>); 5] {
        [
            (NoticeSource::Local, self.local),
            (NoticeSource::Project, self.project),
            (NoticeSource::Flag, self.flag),
            (NoticeSource::Environment, self.environment),
            (NoticeSource::User, self.user),
        ]
    }

    /// Resolve the effective value of `key` and the layer it comes from.
    #[must_use]
    pub fn resolve(self, key: ConfigKey) -> NoticeResolution {
        let (effective, source) = self
            .entries()
            .into_iter()
            .find_map(|(source, value)| value.map(|value| (value, source)))
            .unwrap_or((key.default_value(), NoticeSource::Default));
        NoticeResolution {
            effective,
            source,
            layers: self,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::{LaunchValues, NoticeConfig};
    use crate::id::ProjectUuid;

    const ON: Option<NoticeValue> = Some(NoticeValue::On);
    const OFF: Option<NoticeValue> = Some(NoticeValue::Off);
    const KEY: ConfigKey = ConfigKey::NoticeMdProject;

    fn resolved(layers: NoticeLayers) -> (NoticeValue, NoticeSource) {
        let resolution = layers.resolve(KEY);
        (resolution.effective, resolution.source)
    }

    fn project_config(notice: NoticeConfig) -> ProjectConfig {
        let mut config =
            ProjectConfig::from_toml(&format!("project_uuid = \"{}\"\n", ProjectUuid::new()))
                .expect("parse minimal project config");
        config.notice = notice;
        config
    }

    #[test]
    fn resolver_defaults_to_on_when_no_layer_is_set_for_each_key() {
        for key in ConfigKey::ALL {
            let resolution = NoticeLayers::default().resolve(key);
            assert_eq!(
                (resolution.effective, resolution.source),
                (NoticeValue::On, NoticeSource::Default)
            );
        }
    }

    /// The five layers as (layer name, setter putting `value` on that layer), in precedence order.
    type LayerSetter = fn(&mut NoticeLayers, Option<NoticeValue>);
    const ORDER: [(NoticeSource, LayerSetter); 5] = [
        (NoticeSource::Local, |layers, value| layers.local = value),
        (NoticeSource::Project, |layers, value| {
            layers.project = value
        }),
        (NoticeSource::Flag, |layers, value| layers.flag = value),
        (NoticeSource::Environment, |layers, value| {
            layers.environment = value;
        }),
        (NoticeSource::User, |layers, value| layers.user = value),
    ];

    #[test]
    fn resolver_order_is_local_project_flag_environment_user_default() {
        for (higher_index, (higher, set_higher)) in ORDER.iter().enumerate() {
            for (lower, set_lower) in &ORDER[higher_index + 1..] {
                for (high_value, low_value) in [(OFF, ON), (ON, OFF)] {
                    let mut layers = NoticeLayers::default();
                    set_higher(&mut layers, high_value);
                    set_lower(&mut layers, low_value);
                    let resolution = layers.resolve(KEY);
                    assert_eq!(
                        (Some(resolution.effective), resolution.source),
                        (high_value, *higher),
                        "{higher:?} must beat {lower:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn entries_list_every_layer_with_its_value_in_precedence_order() {
        let layers = NoticeLayers {
            local: ON,
            project: OFF,
            flag: ON,
            environment: OFF,
            user: None,
        };
        assert_eq!(
            layers.entries(),
            [
                (NoticeSource::Local, ON),
                (NoticeSource::Project, OFF),
                (NoticeSource::Flag, ON),
                (NoticeSource::Environment, OFF),
                (NoticeSource::User, None),
            ]
        );
    }

    #[test]
    fn resolver_local_off_wins_over_every_combination_of_the_other_layers() {
        let values = [None, ON, OFF];
        for project in values {
            for flag in values {
                for environment in values {
                    for user in values {
                        let layers = NoticeLayers {
                            local: OFF,
                            project,
                            flag,
                            environment,
                            user,
                        };
                        assert_eq!(
                            resolved(layers),
                            (NoticeValue::Off, NoticeSource::Local),
                            "layers: {layers:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resolver_project_on_wins_over_a_launch_off() {
        let layers = NoticeLayers {
            project: ON,
            flag: OFF,
            environment: OFF,
            ..Default::default()
        };
        assert_eq!(resolved(layers), (NoticeValue::On, NoticeSource::Project));
    }

    #[test]
    fn resolver_reports_the_deciding_source_and_every_layer() {
        let layers = NoticeLayers {
            environment: OFF,
            user: ON,
            ..Default::default()
        };
        let resolution = layers.resolve(KEY);
        assert_eq!(resolution.source, NoticeSource::Environment);
        assert_eq!(resolution.layers, layers);
    }

    #[test]
    fn collect_reads_each_layer_of_the_requested_key_only() {
        let mut local = LocalConfig::default();
        local
            .notice
            .set(ConfigKey::NoticeMdProject, NoticeValue::Off);
        let mut project = project_config(NoticeConfig::default());
        project
            .notice
            .set(ConfigKey::NoticeMdUser, NoticeValue::Off);
        let mut user = UserConfig::default();
        user.notice.set(ConfigKey::NoticeMdProject, NoticeValue::On);
        let launch = NoticeLaunch {
            md_project: LaunchValues {
                flag: OFF,
                environment: None,
            },
            md_user: LaunchValues {
                flag: None,
                environment: ON,
            },
        };

        let project_key = NoticeLayers::collect(
            ConfigKey::NoticeMdProject,
            Some(&local),
            Some(&project),
            &user,
            &launch,
        );
        let user_key = NoticeLayers::collect(
            ConfigKey::NoticeMdUser,
            Some(&local),
            Some(&project),
            &user,
            &launch,
        );

        assert_eq!(
            project_key,
            NoticeLayers {
                local: OFF,
                project: None,
                flag: OFF,
                environment: None,
                user: ON,
            }
        );
        assert_eq!(
            user_key,
            NoticeLayers {
                local: None,
                project: OFF,
                flag: None,
                environment: ON,
                user: None,
            }
        );
    }

    #[test]
    fn collect_without_a_project_reads_the_launch_and_user_layers_only() {
        let mut user = UserConfig::default();
        user.notice.set(KEY, NoticeValue::Off);

        let layers = NoticeLayers::collect(KEY, None, None, &user, &NoticeLaunch::default());

        assert_eq!(
            layers,
            NoticeLayers {
                user: OFF,
                ..Default::default()
            }
        );
    }

    #[test]
    fn resolver_resolves_each_key_independently() {
        let mut user = UserConfig::default();
        user.notice
            .set(ConfigKey::NoticeMdProject, NoticeValue::Off);
        let launch = NoticeLaunch::default();

        let project_key =
            NoticeLayers::collect(ConfigKey::NoticeMdProject, None, None, &user, &launch)
                .resolve(ConfigKey::NoticeMdProject);
        let user_key = NoticeLayers::collect(ConfigKey::NoticeMdUser, None, None, &user, &launch)
            .resolve(ConfigKey::NoticeMdUser);

        assert_eq!(project_key.effective, NoticeValue::Off);
        assert_eq!(user_key.effective, NoticeValue::On);
        assert_eq!(user_key.source, NoticeSource::Default);
    }
}
