//! [`NoticeConfig`], the `[notice]` table shared by every configuration file.

use serde::{Deserialize, Serialize};

use super::{MdNoticeConfig, NoticeValue};
use crate::config::ConfigKey;

/// The `[notice]` table of `~/.mmcp/config.toml`, `.mmcp.toml` and `.mmcp.local.toml`.
/// Rendered only when a key is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticeConfig {
    /// Notices about CLAUDE.md files.
    #[serde(default, skip_serializing_if = "MdNoticeConfig::is_empty")]
    pub md: MdNoticeConfig,
}

impl NoticeConfig {
    /// Whether no key is set, so the table is left out of the file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.md.is_empty()
    }

    /// The value of `key` in this table, `None` when it is not set.
    #[must_use]
    pub fn get(&self, key: ConfigKey) -> Option<NoticeValue> {
        *self.slot(key)
    }

    /// Set `key` to `value`; `true` when the stored value changed.
    pub fn set(&mut self, key: ConfigKey, value: NoticeValue) -> bool {
        let slot = self.slot_mut(key);
        let changed = *slot != Some(value);
        *slot = Some(value);
        changed
    }

    /// Remove `key`; `true` when a value was stored.
    pub fn unset(&mut self, key: ConfigKey) -> bool {
        self.slot_mut(key).take().is_some()
    }

    fn slot(&self, key: ConfigKey) -> &Option<NoticeValue> {
        match key {
            ConfigKey::NoticeMdProject => &self.md.project,
            ConfigKey::NoticeMdUser => &self.md.user,
        }
    }

    fn slot_mut(&mut self, key: ConfigKey) -> &mut Option<NoticeValue> {
        match key {
            ConfigKey::NoticeMdProject => &mut self.md.project,
            ConfigKey::NoticeMdUser => &mut self.md.user,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_fresh_table_is_empty_and_holds_no_key() {
        let table = NoticeConfig::default();
        assert!(table.is_empty());
        assert_eq!(table.get(ConfigKey::NoticeMdProject), None);
        assert_eq!(table.get(ConfigKey::NoticeMdUser), None);
    }

    #[test]
    fn each_key_addresses_its_own_value() {
        let mut table = NoticeConfig::default();
        table.set(ConfigKey::NoticeMdProject, NoticeValue::Off);
        assert_eq!(
            table.get(ConfigKey::NoticeMdProject),
            Some(NoticeValue::Off)
        );
        assert_eq!(table.get(ConfigKey::NoticeMdUser), None);
        table.set(ConfigKey::NoticeMdUser, NoticeValue::On);
        assert_eq!(table.get(ConfigKey::NoticeMdUser), Some(NoticeValue::On));
        assert_eq!(
            table.get(ConfigKey::NoticeMdProject),
            Some(NoticeValue::Off)
        );
    }

    #[test]
    fn set_and_unset_report_changed_and_repeat_as_unchanged() {
        let mut table = NoticeConfig::default();
        assert!(table.set(ConfigKey::NoticeMdProject, NoticeValue::Off));
        assert!(!table.set(ConfigKey::NoticeMdProject, NoticeValue::Off));
        assert!(table.set(ConfigKey::NoticeMdProject, NoticeValue::On));
        assert!(table.unset(ConfigKey::NoticeMdProject));
        assert!(!table.unset(ConfigKey::NoticeMdProject));
    }

    #[test]
    fn unsetting_the_last_key_leaves_an_empty_table() {
        let mut table = NoticeConfig::default();
        table.set(ConfigKey::NoticeMdUser, NoticeValue::Off);
        assert!(!table.is_empty());
        table.unset(ConfigKey::NoticeMdUser);
        assert!(table.is_empty());
    }

    #[test]
    fn the_table_round_trips_through_toml() {
        let mut table = NoticeConfig::default();
        table.set(ConfigKey::NoticeMdProject, NoticeValue::Off);
        table.set(ConfigKey::NoticeMdUser, NoticeValue::On);

        let rendered = toml::to_string(&table).expect("render");
        let reparsed: NoticeConfig = toml::from_str(&rendered).expect("reparse");

        assert_eq!(rendered, "[md]\nproject = \"off\"\nuser = \"on\"\n");
        assert_eq!(reparsed, table);
    }

    #[test]
    fn an_unknown_key_is_rejected_at_every_level() {
        assert!(toml::from_str::<NoticeConfig>("[md]\nprojet = \"off\"\n").is_err());
        assert!(toml::from_str::<NoticeConfig>("[mdd]\nproject = \"off\"\n").is_err());
    }

    #[test]
    fn an_invalid_value_is_rejected() {
        assert!(toml::from_str::<NoticeConfig>("[md]\nproject = \"maybe\"\n").is_err());
        assert!(toml::from_str::<NoticeConfig>("[md]\nproject = true\n").is_err());
    }
}
