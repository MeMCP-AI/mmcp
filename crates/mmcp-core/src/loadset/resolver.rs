//! Load-set resolution from a project configuration.

use serde::{Deserialize, Serialize};

use crate::config::ProjectConfig;
use crate::loadset::GroupRef;

/// Ordered list of groups to pull for a session.
///
/// The order is load priority, not alphabetical. Earlier entries take
/// precedence when the same memory name appears in multiple groups:
///
/// 1. The project's own group (keyed by `project_uuid`).
/// 2. The `global` group, unless disabled by
///    `subscriptions.no_default_global`.
/// 3. Language convention groups from `subscriptions.languages` and,
///    if `subscriptions.auto_detect_languages` is enabled, the
///    auto-detected set.
/// 4. Additional groups listed in `subscriptions.groups`.
///
/// Duplicate entries across these buckets are removed while
/// preserving the first occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LoadSet {
    /// Ordered group references. Index 0 is highest priority.
    pub groups: Vec<GroupRef>,
}

impl LoadSet {
    /// True if the load set is empty. Practically only happens when a
    /// caller builds a `LoadSet` by hand; the resolver always emits
    /// at least the project's own group.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Number of groups in the load set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Borrow the groups as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[GroupRef] {
        &self.groups
    }
}

/// Compute the effective [`LoadSet`] for a session.
///
/// `config` is the project's parsed `.mmcp/config.toml`. `detected`
/// is the list of languages discovered by scanning the project tree,
/// which the caller passes in as strings (e.g. `["rust", "python"]`).
/// When `subscriptions.auto_detect_languages` is `false`, `detected`
/// is ignored.
#[must_use]
pub fn resolve_load_set(config: &ProjectConfig, detected: &[String]) -> LoadSet {
    let mut groups: Vec<GroupRef> = Vec::new();

    groups.push(GroupRef::Project(config.project_uuid));

    if !config.subscriptions.no_default_global {
        push_unique(&mut groups, GroupRef::Global);
    }

    for lang in &config.subscriptions.languages {
        push_unique(&mut groups, GroupRef::Language(lang.clone()));
    }

    if config.subscriptions.auto_detect_languages {
        for lang in detected {
            push_unique(&mut groups, GroupRef::Language(lang.clone()));
        }
    }

    for extra in &config.subscriptions.groups {
        push_unique(&mut groups, GroupRef::Named(extra.clone()));
    }

    LoadSet { groups }
}

fn push_unique(groups: &mut Vec<GroupRef>, candidate: GroupRef) {
    if !groups.contains(&candidate) {
        groups.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::{ProjectConfig, SubscriptionsConfig, SyncConfig};
    use crate::id::ProjectUuid;

    fn cfg(sync: Option<SyncConfig>, subscriptions: SubscriptionsConfig) -> ProjectConfig {
        ProjectConfig {
            project_uuid: ProjectUuid::new(),
            project_slug: None,
            sync,
            subscriptions,
        }
    }

    #[test]
    fn minimal_config_loads_project_and_global() {
        let config = cfg(None, SubscriptionsConfig::default());
        let set = resolve_load_set(&config, &[]);
        assert_eq!(set.len(), 2);
        assert_eq!(set.groups[0], GroupRef::Project(config.project_uuid));
        assert_eq!(set.groups[1], GroupRef::Global);
    }

    #[test]
    fn no_default_skips_global() {
        let subs = SubscriptionsConfig {
            no_default_global: true,
            ..Default::default()
        };
        let config = cfg(None, subs);
        let set = resolve_load_set(&config, &[]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.groups[0], GroupRef::Project(config.project_uuid));
    }

    #[test]
    fn explicit_languages_are_loaded_in_order() {
        let subs = SubscriptionsConfig {
            languages: vec!["rust".into(), "python".into()],
            ..Default::default()
        };
        let config = cfg(None, subs);
        let set = resolve_load_set(&config, &[]);
        assert_eq!(
            set.groups[2..],
            [
                GroupRef::Language("rust".into()),
                GroupRef::Language("python".into()),
            ]
        );
    }

    #[test]
    fn auto_detect_adds_detected_languages() {
        let subs = SubscriptionsConfig {
            auto_detect_languages: true,
            ..Default::default()
        };
        let config = cfg(None, subs);
        let set = resolve_load_set(&config, &["rust".into()]);
        assert_eq!(set.groups[2], GroupRef::Language("rust".into()));
    }

    #[test]
    fn auto_detect_disabled_ignores_detected_list() {
        let config = cfg(None, SubscriptionsConfig::default());
        let set = resolve_load_set(&config, &["rust".into()]);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn additional_groups_are_appended_last() {
        let subs = SubscriptionsConfig {
            groups: vec!["team/shared".into()],
            ..Default::default()
        };
        let config = cfg(None, subs);
        let set = resolve_load_set(&config, &[]);
        assert_eq!(
            set.groups.last(),
            Some(&GroupRef::Named("team/shared".into()))
        );
    }

    #[test]
    fn duplicates_are_deduplicated_preserving_first_occurrence() {
        let subs = SubscriptionsConfig {
            languages: vec!["rust".into()],
            auto_detect_languages: true,
            ..Default::default()
        };
        let config = cfg(None, subs);
        let set = resolve_load_set(&config, &["rust".into(), "rust".into()]);
        let rust_count = set
            .groups
            .iter()
            .filter(|g| matches!(g, GroupRef::Language(l) if l == "rust"))
            .count();
        assert_eq!(rust_count, 1);
    }

    #[test]
    fn canonical_names_match_design_document() {
        let uuid = ProjectUuid::new();
        assert_eq!(GroupRef::Global.canonical_name(), "global");
        assert_eq!(GroupRef::Project(uuid).canonical_name(), uuid.to_string());
        assert_eq!(
            GroupRef::Language("rust".into()).canonical_name(),
            "lang/rust"
        );
        assert_eq!(
            GroupRef::Named("team/shared".into()).canonical_name(),
            "team/shared"
        );
    }
}
