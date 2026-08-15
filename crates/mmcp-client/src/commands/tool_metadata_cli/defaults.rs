//! Default values for [`super::tool_metadata`].

/// Namespaced `_meta` advisory keys.
/// See [`super::tool_metadata`]'s doc for the vocabulary each one signals.
pub(super) const META_REQUIRES_PROJECT: &str = "mmcp.requires_project";
pub(super) const META_REQUIRES_SYNC: &str = "mmcp.requires_sync";
pub(super) const META_NETWORK: &str = "mmcp.network";
pub(super) const META_DEBUG_GATED: &str = "mmcp.debug_gated";
pub(super) const META_PROTECTED_GROUP_GATED: &str = "mmcp.protected_group_gated";

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
