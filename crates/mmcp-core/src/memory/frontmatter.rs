//! Typed representation of the TOML frontmatter block that heads each
//! memory file.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::{BumpIntent, FeatureMetadata, MemoryKind};

/// User-visible metadata written in the `+++`-delimited TOML block at
/// the top of a memory file.
///
/// Every field except `name`, `description`, and `kind` is optional.
/// Unknown fields encountered on disk are preserved by the parser and
/// written back out verbatim, so future mmcp versions can add fields
/// without breaking older clients.
///
/// The `version` field is server-managed. Clients do not hand-edit it;
/// the server assigns it at push time using the
/// [`BumpIntent`](crate::memory::BumpIntent) that accompanied the
/// commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    /// Canonical primary key. Assigned once (UUIDv7) at create time
    /// and never changes across renames. Absent on memories written
    /// before FR-028; the migration binary backfills every repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,

    /// Human-readable title displayed in listings and the WebUI.
    pub name: String,

    /// One-line summary used by the AI for relevance inference.
    pub description: String,

    /// Behavior classification.
    pub kind: MemoryKind,

    /// Whether this memory must be read at least once per session.
    #[serde(default)]
    pub mandatory: bool,

    /// Current published version. Managed by the server; ignored on
    /// incoming client edits.
    #[serde(default)]
    pub version: Option<semver::Version>,

    /// Free-form classification tags.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Bump intent hint for the next version assignment. Editors set
    /// this; the server consumes and clears it at push time.
    #[serde(default)]
    pub bump_intent: Option<BumpIntent>,

    /// Structured metadata populated only when `kind == MemoryKind::Feature`.
    /// Carries the feature-request lifecycle (status + cross-ref slugs)
    /// so FR tools avoid re-parsing the body to classify memories.
    /// Absent on every non-FR memory; the TOML serializer skips the
    /// field when unset so unrelated memories keep their existing
    /// wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<FeatureMetadata>,
}

impl MemoryFrontmatter {
    /// Build a frontmatter record with sensible defaults for every
    /// optional field. Call sites set the three required fields
    /// (name / description / kind) and reach for the `with_*`
    /// helpers only when they need to override a default. Keeping
    /// this constructor is load-bearing: it is the single place
    /// that has to learn about a new optional frontmatter field,
    /// so "add one field to MemoryFrontmatter" stays a one-file
    /// change across the workspace.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        kind: MemoryKind,
    ) -> Self {
        Self {
            id: None,
            name: name.into(),
            description: description.into(),
            kind,
            mandatory: false,
            version: None,
            tags: Vec::new(),
            bump_intent: None,
            feature: None,
        }
    }

    /// Pin the canonical UUIDv7 primary key (FR-028).
    #[must_use]
    pub fn with_id(mut self, id: uuid::Uuid) -> Self {
        self.id = Some(id);
        self
    }

    /// Flip the mandatory-read flag.
    #[must_use]
    pub fn with_mandatory(mut self, mandatory: bool) -> Self {
        self.mandatory = mandatory;
        self
    }

    /// Replace the tag set.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Attach the feature-request metadata block. Used by the FR
    /// tooling layer; ordinary writers leave this absent.
    #[must_use]
    pub fn with_feature(mut self, feature: FeatureMetadata) -> Self {
        self.feature = Some(feature);
        self
    }

    /// Override the server-managed version. Production writers
    /// leave this at the default `None`; tests and migrations may
    /// need to pin it explicitly.
    #[must_use]
    pub fn with_version(mut self, version: Option<semver::Version>) -> Self {
        self.version = version;
        self
    }

    /// Override the `bump_intent` hint. Editors set this before a
    /// push so the server can assign the next semantic version.
    #[must_use]
    pub fn with_bump_intent(mut self, bump_intent: Option<BumpIntent>) -> Self {
        self.bump_intent = bump_intent;
        self
    }
}
