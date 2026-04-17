//! `.mmcp.toml` group manifest committed at the root of each group
//! repository.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::id::GroupId;
use crate::manifest::ManifestError;

/// File name the manifest is committed under at the repo root.
pub const MANIFEST_FILENAME: &str = ".mmcp.toml";

/// Current schema version produced by this crate.
///
/// Older clients that encounter a manifest with a higher value
/// should refuse to parse it rather than silently ignore fields
/// they do not understand.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Self-describing manifest for a group git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupManifest {
    /// Schema version of this manifest. Must equal
    /// [`MANIFEST_SCHEMA_VERSION`] on disk; readers that see a
    /// higher value should return
    /// [`ManifestError::UnsupportedSchema`].
    pub schema_version: u32,

    /// Stable identifier for the group. Matches the `groups.id`
    /// column on the server side.
    pub group_id: GroupId,

    /// Human-readable slug. Not guaranteed to be unique across
    /// owners in isolation; the owner hint qualifies it.
    pub slug: String,

    /// Optional display name shown in WebUI listings.
    #[serde(default)]
    pub display_name: Option<String>,

    /// Hint about who currently owns this group. Used for disaster
    /// recovery; the server is still the authoritative owner-of-record.
    pub owner: GroupOwnerHint,

    /// When true, any mutation of a memory in this group requires
    /// user-visible confirmation. Stored in the manifest so the
    /// flag travels with the repo on pull: a project that syncs a
    /// protected group inherits the protection automatically
    /// without any per-project opt-in.
    ///
    /// Serde-default-false + skip-when-false keeps pre-flag
    /// manifests parsing unchanged and omits the field from
    /// rendered manifests that don't need it, so the wire shape
    /// stays minimal.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub protected: bool,

    /// Creation time of the group on the original server, as
    /// milliseconds since the Unix epoch.
    pub created_at: i64,
}

/// Owner hint stored inside the manifest.
///
/// The server is authoritative for the real owner; this enum only
/// captures enough information to reconstruct a sensible default
/// during disaster recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum GroupOwnerHint {
    User(Uuid),
    Org(Uuid),
}

impl GroupManifest {
    /// Build a fresh manifest for a user-owned group created now.
    ///
    /// `protected` defaults to false; callers opt a group into
    /// protection via field assignment on the returned manifest
    /// before `create_group_repo`, or via a future
    /// `set_protected` path on an existing repo.
    #[must_use]
    pub fn new_user_owned(group_id: GroupId, slug: impl Into<String>, owner: Uuid) -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            group_id,
            slug: slug.into(),
            display_name: None,
            owner: GroupOwnerHint::User(owner),
            protected: false,
            created_at: Timestamp::now().as_millisecond(),
        }
    }

    /// Build a fresh manifest for an org-owned group created now.
    #[must_use]
    pub fn new_org_owned(group_id: GroupId, slug: impl Into<String>, owner: Uuid) -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            group_id,
            slug: slug.into(),
            display_name: None,
            owner: GroupOwnerHint::Org(owner),
            protected: false,
            created_at: Timestamp::now().as_millisecond(),
        }
    }

    /// Parse a manifest from TOML text.
    ///
    /// Returns [`ManifestError::UnsupportedSchema`] if the embedded
    /// `schema_version` exceeds the value this build was compiled
    /// against. Lower values are accepted so older manifests still
    /// load without a migration step.
    pub fn from_toml(source: &str) -> Result<Self, ManifestError> {
        let parsed: Self = toml::from_str(source)?;
        if parsed.schema_version > MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                found: parsed.schema_version,
                supported: MANIFEST_SCHEMA_VERSION,
            });
        }
        Ok(parsed)
    }

    /// Render the manifest to TOML text suitable for committing to
    /// the repo root.
    pub fn to_toml(&self) -> Result<String, ManifestError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_owned_manifest_round_trips() {
        let manifest =
            GroupManifest::new_user_owned(GroupId::new(), "team-rust", Uuid::now_v7());
        let text = manifest.to_toml().unwrap();
        let parsed = GroupManifest::from_toml(&text).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn org_owned_manifest_round_trips() {
        let manifest = GroupManifest::new_org_owned(GroupId::new(), "shared", Uuid::now_v7());
        let text = manifest.to_toml().unwrap();
        let parsed = GroupManifest::from_toml(&text).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn missing_required_fields_is_rejected() {
        let text = r#"schema_version = 1"#;
        assert!(GroupManifest::from_toml(text).is_err());
    }

    #[test]
    fn newer_schema_version_is_rejected_explicitly() {
        let owner = Uuid::now_v7();
        let mut manifest = GroupManifest::new_user_owned(GroupId::new(), "future", owner);
        manifest.schema_version = MANIFEST_SCHEMA_VERSION + 1;
        let text = manifest.to_toml().unwrap();
        let err = GroupManifest::from_toml(&text).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedSchema { .. }));
    }

    #[test]
    fn manifest_filename_is_dot_mmcp_toml() {
        assert_eq!(MANIFEST_FILENAME, ".mmcp.toml");
    }

    #[test]
    fn manifest_missing_protected_parses_as_false() {
        // Pre-flag manifests on disk must keep parsing. The field's
        // serde-default keeps the round trip backward-compatible.
        // Use a freshly-rendered manifest with the field stripped
        // rather than hand-typing TOML — avoids drifting on
        // GroupId's wire format.
        let manifest =
            GroupManifest::new_user_owned(GroupId::new(), "legacy", Uuid::now_v7());
        let text = manifest.to_toml().unwrap();
        assert!(
            !text.contains("protected"),
            "seed manifest must not include the new field (precondition)",
        );
        let parsed =
            GroupManifest::from_toml(&text).expect("pre-flag manifest must parse");
        assert!(!parsed.protected);
    }

    #[test]
    fn manifest_with_protected_true_round_trips() {
        let mut manifest =
            GroupManifest::new_user_owned(GroupId::new(), "global", Uuid::now_v7());
        manifest.protected = true;
        let text = manifest.to_toml().unwrap();
        assert!(
            text.contains("protected = true"),
            "rendered manifest must serialize the flag when set; got:\n{text}",
        );
        let parsed = GroupManifest::from_toml(&text).unwrap();
        assert!(parsed.protected);
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn manifest_with_protected_false_omits_the_field_on_serialize() {
        // skip_serializing_if keeps the wire shape minimal; a group
        // that has never been opted into protection produces no
        // `protected` line.
        let manifest =
            GroupManifest::new_user_owned(GroupId::new(), "team-rust", Uuid::now_v7());
        let text = manifest.to_toml().unwrap();
        assert!(
            !text.contains("protected"),
            "unprotected manifest must not emit the field; got:\n{text}",
        );
    }
}
