//! [`ClaudeMdLayers`], the per-layer values the suggestion resolves from.

use super::{ClaudeMdResolution, ClaudeMdSource, ClaudeMdSuggestion};
use crate::config::{ProjectConfig, UserConfig};

/// Value of every layer, `None` where a layer sets nothing.
/// Precedence, highest first: `user_project`, `project`, `flag`, `environment`, `user`, then the default.
/// The launch layers rank below both per-project layers, so a recorded refusal is never undone by a launch value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaudeMdLayers {
    /// The user's entry for this project in `~/.mmcp/config.toml`.
    pub user_project: Option<ClaudeMdSuggestion>,
    /// The project's `.mmcp.toml`.
    pub project: Option<ClaudeMdSuggestion>,
    /// The launch flag of the serving process.
    pub flag: Option<ClaudeMdSuggestion>,
    /// The launch environment variable of the serving process.
    pub environment: Option<ClaudeMdSuggestion>,
    /// The user's setting for every project in `~/.mmcp/config.toml`.
    pub user: Option<ClaudeMdSuggestion>,
}

impl ClaudeMdLayers {
    /// Persisted layers read from the loaded configs, the launch layers unset.
    /// A table holding an invalid value counts as unset for its layer.
    #[must_use]
    pub fn from_persisted_configs(project: Option<&ProjectConfig>, user: &UserConfig) -> Self {
        Self {
            user_project: project.and_then(|config| {
                user.project_claude_md(config.project_uuid)
                    .and_then(|table| table.suggestion().ok().flatten())
            }),
            project: project.and_then(|config| config.claude_md.suggestion().ok().flatten()),
            flag: None,
            environment: None,
            user: user.claude_md.suggestion().ok().flatten(),
        }
    }

    /// Resolve the effective suggestion and the layer it comes from.
    #[must_use]
    pub fn resolve(self) -> ClaudeMdResolution {
        let (effective, source) = [
            (self.user_project, ClaudeMdSource::UserProject),
            (self.project, ClaudeMdSource::Project),
            (self.flag, ClaudeMdSource::Flag),
            (self.environment, ClaudeMdSource::Environment),
            (self.user, ClaudeMdSource::User),
        ]
        .into_iter()
        .find_map(|(value, source)| value.map(|value| (value, source)))
        .unwrap_or((ClaudeMdSuggestion::Suggest, ClaudeMdSource::Default));
        ClaudeMdResolution {
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
    use crate::id::ProjectUuid;

    const SUGGEST: Option<ClaudeMdSuggestion> = Some(ClaudeMdSuggestion::Suggest);
    const DECLINE: Option<ClaudeMdSuggestion> = Some(ClaudeMdSuggestion::Decline);

    fn resolved(layers: ClaudeMdLayers) -> (ClaudeMdSuggestion, ClaudeMdSource) {
        let resolution = layers.resolve();
        (resolution.effective, resolution.source)
    }

    #[test]
    fn resolver_defaults_to_suggest_when_no_layer_is_set() {
        assert_eq!(
            resolved(ClaudeMdLayers::default()),
            (ClaudeMdSuggestion::Suggest, ClaudeMdSource::Default)
        );
    }

    #[test]
    fn resolver_user_decline_applies_to_every_project() {
        let layers = ClaudeMdLayers {
            user: DECLINE,
            ..Default::default()
        };
        assert_eq!(
            resolved(layers),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::User)
        );
    }

    #[test]
    fn resolver_project_suggest_overrides_user_decline() {
        let layers = ClaudeMdLayers {
            project: SUGGEST,
            user: DECLINE,
            ..Default::default()
        };
        assert_eq!(
            resolved(layers),
            (ClaudeMdSuggestion::Suggest, ClaudeMdSource::Project)
        );
    }

    #[test]
    fn resolver_project_decline_overrides_user_suggest() {
        let layers = ClaudeMdLayers {
            project: DECLINE,
            user: SUGGEST,
            ..Default::default()
        };
        assert_eq!(
            resolved(layers),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::Project)
        );
    }

    #[test]
    fn resolver_user_project_overrides_project_in_both_directions() {
        let decline_over_suggest = ClaudeMdLayers {
            user_project: DECLINE,
            project: SUGGEST,
            ..Default::default()
        };
        assert_eq!(
            resolved(decline_over_suggest),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::UserProject)
        );
        let suggest_over_decline = ClaudeMdLayers {
            user_project: SUGGEST,
            project: DECLINE,
            ..Default::default()
        };
        assert_eq!(
            resolved(suggest_over_decline),
            (ClaudeMdSuggestion::Suggest, ClaudeMdSource::UserProject)
        );
    }

    #[test]
    fn resolver_user_project_decline_wins_over_every_launch_and_persisted_combination() {
        let values = [None, SUGGEST, DECLINE];
        for project in values {
            for flag in values {
                for environment in values {
                    for user in values {
                        let layers = ClaudeMdLayers {
                            user_project: DECLINE,
                            project,
                            flag,
                            environment,
                            user,
                        };
                        assert_eq!(
                            resolved(layers),
                            (ClaudeMdSuggestion::Decline, ClaudeMdSource::UserProject),
                            "layers: {layers:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resolver_project_suggest_wins_over_a_launch_decline() {
        let layers = ClaudeMdLayers {
            project: SUGGEST,
            flag: DECLINE,
            environment: DECLINE,
            ..Default::default()
        };
        assert_eq!(
            resolved(layers),
            (ClaudeMdSuggestion::Suggest, ClaudeMdSource::Project)
        );
    }

    #[test]
    fn resolver_flag_overrides_environment_and_both_override_user() {
        let flag_over_environment = ClaudeMdLayers {
            flag: DECLINE,
            environment: SUGGEST,
            user: SUGGEST,
            ..Default::default()
        };
        assert_eq!(
            resolved(flag_over_environment),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::Flag)
        );
        let environment_over_user = ClaudeMdLayers {
            environment: DECLINE,
            user: SUGGEST,
            ..Default::default()
        };
        assert_eq!(
            resolved(environment_over_user),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::Environment)
        );
    }

    #[test]
    fn resolver_reports_the_deciding_source_for_each_layer_and_keeps_every_layer_value() {
        let layers = ClaudeMdLayers {
            user_project: None,
            project: None,
            flag: None,
            environment: DECLINE,
            user: SUGGEST,
        };
        let resolution = layers.resolve();
        assert_eq!(resolution.source, ClaudeMdSource::Environment);
        assert_eq!(resolution.layers, layers);
    }

    #[test]
    fn resolver_counts_an_invalid_table_as_unset() {
        let uuid = ProjectUuid::new();
        let project = ProjectConfig::from_toml(&format!(
            "project_uuid = \"{uuid}\"\n[claude_md]\nproject_file_suggestion = \"maybe\"\n"
        ))
        .expect("an invalid claude_md value never fails the project config");
        let user = UserConfig::from_toml("[claude_md]\nproject_file_suggestion = \"decline\"\n")
            .expect("parse user config");

        let layers = ClaudeMdLayers::from_persisted_configs(Some(&project), &user);

        assert_eq!(layers.project, None);
        assert_eq!(
            resolved(layers),
            (ClaudeMdSuggestion::Decline, ClaudeMdSource::User)
        );
    }

    #[test]
    fn persisted_layers_read_the_user_entry_of_the_project_uuid_only() {
        let uuid = ProjectUuid::new();
        let other = ProjectUuid::new();
        let project =
            ProjectConfig::from_toml(&format!("project_uuid = \"{uuid}\"\n")).expect("parse");
        let mut user = UserConfig::default();
        user.set_project_claude_md_suggestion(other, Some(ClaudeMdSuggestion::Decline));

        let layers = ClaudeMdLayers::from_persisted_configs(Some(&project), &user);
        assert_eq!(layers.user_project, None);

        user.set_project_claude_md_suggestion(uuid, Some(ClaudeMdSuggestion::Decline));
        let layers = ClaudeMdLayers::from_persisted_configs(Some(&project), &user);
        assert_eq!(layers.user_project, DECLINE);
    }

    #[test]
    fn persisted_layers_without_a_project_read_the_global_switch_only() {
        let mut user = UserConfig::default();
        user.set_claude_md_suggestion(Some(ClaudeMdSuggestion::Decline));
        let layers = ClaudeMdLayers::from_persisted_configs(None, &user);
        assert_eq!(
            layers,
            ClaudeMdLayers {
                user: DECLINE,
                ..Default::default()
            }
        );
    }
}
