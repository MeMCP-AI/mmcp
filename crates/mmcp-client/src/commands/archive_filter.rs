//! Shared clap filter flags for `mmcp export` and `mmcp import`.
//!
//! Flattened into both commands so the facet surface stays identical:
//! include / exclude by slug, kind, and tag; any-or-all tag matching;
//! full-text search; and a mandatory tri-state. `to_filter` validates
//! kind names and folds the two boolean mandatory flags into the
//! store's tri-state.

use anyhow::Result;
use mmcp_core::memory::MemoryKind;
use mmcp_store::{MemoryFilter, parse_memory_kind};

#[derive(clap::Args, Clone, Default)]
pub struct MemoryFilterArgs {
    /// Include only memories with these slugs (repeatable).
    #[arg(long = "memory")]
    pub memory: Vec<String>,

    /// Exclude memories with these slugs (repeatable).
    #[arg(long = "exclude-memory")]
    pub exclude_memory: Vec<String>,

    /// Include only these kinds: rule, snapshot, log, reference,
    /// scratch, feature, issue (repeatable).
    #[arg(long = "kind")]
    pub kind: Vec<String>,

    /// Exclude these kinds (repeatable).
    #[arg(long = "exclude-kind")]
    pub exclude_kind: Vec<String>,

    /// Include only memories carrying these tags (repeatable).
    #[arg(long = "tag")]
    pub tag: Vec<String>,

    /// Require every `--tag` rather than any one of them.
    #[arg(long = "all-tags", default_value_t = false)]
    pub all_tags: bool,

    /// Exclude memories carrying these tags (repeatable).
    #[arg(long = "exclude-tag")]
    pub exclude_tag: Vec<String>,

    /// Case-insensitive substring over name, description, tags, slug,
    /// and body.
    #[arg(long)]
    pub search: Option<String>,

    /// Keep only mandatory memories.
    #[arg(
        long = "mandatory",
        default_value_t = false,
        conflicts_with = "non_mandatory"
    )]
    pub mandatory: bool,

    /// Keep only non-mandatory memories.
    #[arg(long = "non-mandatory", default_value_t = false)]
    pub non_mandatory: bool,

    /// Keep only memories that carry cross-references.
    #[arg(long = "has-refs", default_value_t = false, conflicts_with = "no_refs")]
    pub has_refs: bool,

    /// Keep only memories with no cross-references.
    #[arg(long = "no-refs", default_value_t = false)]
    pub no_refs: bool,
}

impl MemoryFilterArgs {
    /// True when no facet is set.
    pub fn is_empty(&self) -> bool {
        self.memory.is_empty()
            && self.exclude_memory.is_empty()
            && self.kind.is_empty()
            && self.exclude_kind.is_empty()
            && self.tag.is_empty()
            && self.exclude_tag.is_empty()
            && self.search.is_none()
            && !self.mandatory
            && !self.non_mandatory
            && !self.has_refs
            && !self.no_refs
    }

    /// Build the store-level filter, validating kind names.
    pub fn to_filter(&self) -> Result<MemoryFilter> {
        let mandatory = match (self.mandatory, self.non_mandatory) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        };
        let has_refs = match (self.has_refs, self.no_refs) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        };
        Ok(MemoryFilter {
            slugs: self.memory.clone(),
            exclude_slugs: self.exclude_memory.clone(),
            kinds: parse_kinds(&self.kind)?,
            exclude_kinds: parse_kinds(&self.exclude_kind)?,
            tags: self.tag.clone(),
            require_all_tags: self.all_tags,
            exclude_tags: self.exclude_tag.clone(),
            search: self.search.clone(),
            mandatory,
            has_refs,
        })
    }
}

fn parse_kinds(values: &[String]) -> Result<Vec<MemoryKind>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(parse_memory_kind(value)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use strum::VariantArray as _;

    use super::*;

    #[test]
    fn parse_kinds_rejection_names_every_memory_kind() {
        // The rejection message is built from `MemoryKind::VARIANTS` at the point of failure.
        // A kind added later still appears here without touching this file.
        let err = parse_kinds(&["bogus".to_string()]).expect_err("unknown kind must be rejected");
        let message = err.to_string();
        for kind in MemoryKind::VARIANTS {
            assert!(
                message.contains(kind.as_str()),
                "expected '{}' in rejection message, got: {message}",
                kind.as_str()
            );
        }
    }

    #[test]
    fn parse_kinds_rejection_preserves_the_typed_source() {
        let err = parse_kinds(&["bogus".to_string()]).expect_err("unknown kind must be rejected");
        assert!(
            err.downcast_ref::<mmcp_core::memory::MemoryKindParseError>()
                .is_some(),
            "rejection must carry the typed MemoryKindParseError, not a stringified copy"
        );
    }
}
