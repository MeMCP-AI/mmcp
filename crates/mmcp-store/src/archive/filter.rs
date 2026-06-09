//! Multi-facet memory filter shared by archive export and import.
//!
//! A memory passes when it satisfies every set facet (AND across
//! facets). Each facet supports include and exclude; tag inclusion is
//! any-of by default or all-of when `require_all_tags` is set. Search
//! is full-text over name, description, tags, slug, and body. An empty
//! filter matches everything, so the "all memories" path pays no cost.

use mmcp_core::memory::{MemoryFrontmatter, MemoryKind};

/// Selection facets applied to the memories an archive op touches.
///
/// Default-derived; new facets are additive fields so callers that
/// build it field-by-field keep compiling.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Include only these slugs (empty = no slug constraint).
    pub slugs: Vec<String>,
    /// Exclude these slugs.
    pub exclude_slugs: Vec<String>,
    /// Include only these kinds (empty = any kind).
    pub kinds: Vec<MemoryKind>,
    /// Exclude these kinds.
    pub exclude_kinds: Vec<MemoryKind>,
    /// Include memories carrying these tags (any-of, or all-of when
    /// `require_all_tags`). Empty = no tag-include constraint.
    pub tags: Vec<String>,
    /// Require every tag in `tags` rather than any one of them.
    pub require_all_tags: bool,
    /// Exclude memories carrying any of these tags.
    pub exclude_tags: Vec<String>,
    /// Case-insensitive substring matched against name + description +
    /// tags + slug + body. `None` = no text constraint.
    pub search: Option<String>,
    /// Restrict to mandatory (`Some(true)`) or non-mandatory
    /// (`Some(false)`) memories. `None` = either.
    pub mandatory: Option<bool>,
    /// Restrict to memories that carry cross-references (`Some(true)`)
    /// or carry none (`Some(false)`). `None` = either.
    pub has_refs: Option<bool>,
}

impl MemoryFilter {
    /// True when no facet is set, so the filter matches everything and
    /// callers can skip parsing frontmatter and body entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slugs.is_empty()
            && self.exclude_slugs.is_empty()
            && self.kinds.is_empty()
            && self.exclude_kinds.is_empty()
            && self.tags.is_empty()
            && self.exclude_tags.is_empty()
            && self.search.is_none()
            && self.mandatory.is_none()
            && self.has_refs.is_none()
    }

    /// Whether a memory at `slug` with frontmatter `fm` and `body`
    /// passes every set facet.
    #[must_use]
    pub fn matches(&self, slug: &str, fm: &MemoryFrontmatter, body: &str) -> bool {
        if !self.slugs.is_empty() && !self.slugs.iter().any(|s| s == slug) {
            return false;
        }
        if self.exclude_slugs.iter().any(|s| s == slug) {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&fm.kind) {
            return false;
        }
        if self.exclude_kinds.contains(&fm.kind) {
            return false;
        }
        if !self.tags.is_empty() {
            let has = |t: &String| fm.tags.iter().any(|ft| ft == t);
            let ok = if self.require_all_tags {
                self.tags.iter().all(has)
            } else {
                self.tags.iter().any(has)
            };
            if !ok {
                return false;
            }
        }
        if self.exclude_tags.iter().any(|t| fm.tags.iter().any(|ft| ft == t)) {
            return false;
        }
        if let Some(want) = self.mandatory
            && fm.mandatory != want
        {
            return false;
        }
        if let Some(want) = self.has_refs
            && !fm.refs.is_empty() != want
        {
            return false;
        }
        if let Some(query) = &self.search {
            let needle = query.to_lowercase();
            let haystack = format!(
                "{} {} {} {} {}",
                fm.name,
                fm.description,
                fm.tags.join(" "),
                slug,
                body
            )
            .to_lowercase();
            if !haystack.contains(&needle) {
                return false;
            }
        }
        true
    }
}

/// Parse a memory-kind facet string (accepts every kind, including
/// `feature`/`fr` and `issue`, unlike the create-time `parse_kind`
/// which is limited to the author-facing kinds). Returns `None` for an
/// unknown kind so callers can reject the input with a clear message.
#[must_use]
pub fn parse_memory_kind(value: &str) -> Option<MemoryKind> {
    match value.trim().to_lowercase().as_str() {
        "rule" => Some(MemoryKind::Rule),
        "snapshot" => Some(MemoryKind::Snapshot),
        "log" => Some(MemoryKind::Log),
        "reference" => Some(MemoryKind::Reference),
        "scratch" => Some(MemoryKind::Scratch),
        "feature" | "fr" => Some(MemoryKind::Feature),
        "issue" => Some(MemoryKind::Issue),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(name: &str, kind: MemoryKind, tags: &[&str], mandatory: bool) -> MemoryFrontmatter {
        let mut f = MemoryFrontmatter::new(name.to_string(), "desc".to_string(), kind);
        f.tags = tags.iter().map(|t| (*t).to_string()).collect();
        f.mandatory = mandatory;
        f
    }

    #[test]
    fn empty_filter_matches_everything() {
        let filter = MemoryFilter::default();
        assert!(filter.is_empty());
        assert!(filter.matches("any", &fm("n", MemoryKind::Rule, &[], false), "body"));
    }

    #[test]
    fn include_and_exclude_compose_with_and() {
        let filter = MemoryFilter {
            kinds: vec![MemoryKind::Rule],
            exclude_kinds: vec![MemoryKind::Log],
            tags: vec!["keep".to_string()],
            exclude_tags: vec!["wip".to_string()],
            exclude_slugs: vec!["skip-me".to_string()],
            mandatory: Some(true),
            ..Default::default()
        };
        assert!(filter.matches("s", &fm("n", MemoryKind::Rule, &["keep"], true), ""));
        assert!(!filter.matches("skip-me", &fm("n", MemoryKind::Rule, &["keep"], true), ""));
        assert!(!filter.matches("s", &fm("n", MemoryKind::Log, &["keep"], true), ""));
        assert!(!filter.matches("s", &fm("n", MemoryKind::Rule, &["other"], true), ""));
        assert!(!filter.matches("s", &fm("n", MemoryKind::Rule, &["keep", "wip"], true), ""));
        assert!(!filter.matches("s", &fm("n", MemoryKind::Rule, &["keep"], false), ""));
    }

    #[test]
    fn require_all_tags_demands_every_tag() {
        let any = MemoryFilter {
            tags: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        };
        let all = MemoryFilter {
            tags: vec!["a".to_string(), "b".to_string()],
            require_all_tags: true,
            ..Default::default()
        };
        let one = fm("n", MemoryKind::Rule, &["a"], false);
        assert!(any.matches("s", &one, ""));
        assert!(!all.matches("s", &one, ""));
        let both = fm("n", MemoryKind::Rule, &["a", "b"], false);
        assert!(all.matches("s", &both, ""));
    }

    #[test]
    fn search_matches_body_and_slug_case_insensitively() {
        let filter = MemoryFilter {
            search: Some("needle".to_string()),
            ..Default::default()
        };
        assert!(filter.matches("s", &fm("n", MemoryKind::Rule, &[], false), "has a NEEDLE here"));
        assert!(filter.matches("path/needle", &fm("n", MemoryKind::Rule, &[], false), "body"));
        assert!(!filter.matches("s", &fm("n", MemoryKind::Rule, &[], false), "nothing"));
    }

    #[test]
    fn parse_memory_kind_covers_every_kind() {
        assert_eq!(parse_memory_kind("rule"), Some(MemoryKind::Rule));
        assert_eq!(parse_memory_kind("Feature"), Some(MemoryKind::Feature));
        assert_eq!(parse_memory_kind("fr"), Some(MemoryKind::Feature));
        assert_eq!(parse_memory_kind("issue"), Some(MemoryKind::Issue));
        assert_eq!(parse_memory_kind("nope"), None);
    }
}
