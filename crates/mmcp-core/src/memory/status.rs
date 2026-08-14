//! Shared `Status` trait for tracker-flavored memory kinds.
//!
//! Both `FeatureStatus` (existing) and the upcoming `IssueStatus`
//! carry the same operational surface: a stable lowercase string
//! form, a default-hidden classifier (so listings can hide
//! terminal-ish variants without each kind reinventing the
//! predicate), and a `parse` round-trip from the wire form.
//!
//! This trait lives in its own concern-named module rather than
//! under either kind's submodule. Generic helpers that need to
//! filter or display tracker-status values consume `T: Status`
//! instead of pattern-matching per kind. Each enum redeclares its
//! own variants: Rust enums cannot be extended, but the behaviour
//! contract is one place.

/// Behaviour contract every tracker-flavored status enum implements.
///
/// `Self: Copy + Eq + 'static` mirrors the constraints on the
/// existing `FeatureStatus`; tracker statuses are tiny C-style
/// enums so the bound is incidental in practice. The associated
/// `ParseError` lets each impl raise its own typed error from
/// [`Status::parse`] without going through a stringified bridge.
pub trait Status: Copy + Eq + std::hash::Hash + 'static {
    /// Typed parse error this status enum raises from
    /// [`Status::parse`]. Each impl declares its own so the wire
    /// shape carries the offending input verbatim.
    type ParseError: std::error::Error;

    /// Lowercase wire string matching the serde `snake_case`
    /// serialization. Used for CLI rendering, log output, and
    /// MCP error payloads.
    fn as_str(self) -> &'static str;

    /// Whether this variant is hidden from the default listing
    /// when no explicit status filter is applied. The kind chooses
    /// which variants are "terminal-ish" (work landed, won't fix,
    /// already replaced) so listings stay actionable without
    /// reinventing the predicate at every call site.
    fn is_default_hidden(self) -> bool;

    /// Every variant in declaration order. Used by listing UIs and
    /// validators that need to enumerate the status space.
    fn all() -> &'static [Self];

    /// Parse the lowercase wire form back into a variant.
    fn parse(raw: &str) -> Result<Self, Self::ParseError>;
}
