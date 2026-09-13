//! Default values for `registry::tool_metadata`.

/// Namespaced `_meta` advisory keys.
/// See `registry::tool_metadata`'s doc for the vocabulary each one signals.
pub(super) const META_REQUIRES_PROJECT: &str = "mmcp.requires_project";
pub(super) const META_REQUIRES_SYNC: &str = "mmcp.requires_sync";
pub(super) const META_NETWORK: &str = "mmcp.network";
pub(super) const META_DEBUG_GATED: &str = "mmcp.debug_gated";
pub(super) const META_PROTECTED_GROUP_GATED: &str = "mmcp.protected_group_gated";

/// `_meta` key naming the calling client's own maximum inline tool-result size, in characters.
/// Anthropic-namespaced: this key is a Claude Code convention, not an mmcp one.
pub(super) const META_ANTHROPIC_MAX_RESULT_SIZE_CHARS: &str = "anthropic/maxResultSizeChars";

/// Claude Code's documented hard ceiling on a single tool result, in characters.
/// `read_memory` declares this via [`META_ANTHROPIC_MAX_RESULT_SIZE_CHARS`].
/// A client honoring the key avoids side-filing a body up to this size.
/// `mmcp_core::memory::MAX_BODY_LENGTH` (589,824 bytes) is the real stored-body ceiling and exceeds this value,
/// so a body between the two still gets saved to a file despite the declared hint.
pub(super) const CLAUDE_CODE_MAX_RESULT_SIZE_CHARS: u64 = 500_000;

// Tiny inline-SVG data URIs so the icon ships with the
// binary instead of relying on an external CDN. Each glyph is a
// single emoji rendered as text inside a 16x16 viewBox; clients
// with icon-capable UIs render the emoji at any size.
pub(super) const READ_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F4D6}</text></svg>";
pub(super) const MUTATE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{270F}\u{FE0F}</text></svg>";
pub(super) const FEATURE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F6A9}</text></svg>";
pub(super) const ISSUE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F41E}</text></svg>";
pub(super) const MILESTONE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F3C1}</text></svg>";
pub(super) const DEBUG_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F41B}</text></svg>";
pub(super) const SYNC_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F504}</text></svg>";
