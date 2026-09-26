//! Location of the mmcp-managed fence inside a CLAUDE.md text, whatever its version.

use std::ops::Range;

use super::{
    BEGIN_MARKER_PREFIX, END_MARKER_PREFIX, MARKER_SUFFIX, PartialFenceError, render_block,
};

/// A complete mmcp fence: a begin marker, then an end marker after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence<'text> {
    /// Byte range of the fenced region, begin marker through end marker inclusive.
    pub range: Range<usize>,
    /// Version tag carried by the begin marker.
    pub version: &'text str,
    /// The fenced region itself.
    pub region: &'text str,
}

impl Fence<'_> {
    /// Whether the fenced region is byte-identical to the current render.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.region == render_block()
    }
}

/// One fence marker located in a text.
struct Marker<'text> {
    range: Range<usize>,
    version: &'text str,
}

/// First marker opening with `prefix` at or after byte `from`.
/// Its version tag runs up to the first marker suffix.
fn find_marker<'text>(text: &'text str, from: usize, prefix: &str) -> Option<Marker<'text>> {
    let start = from + text[from..].find(prefix)?;
    let version_start = start + prefix.len();
    let version_end = version_start + text[version_start..].find(MARKER_SUFFIX)?;
    Some(Marker {
        range: start..version_end + MARKER_SUFFIX.len(),
        version: &text[version_start..version_end],
    })
}

/// Locate the mmcp fence of any version in `text`.
/// The fence runs from the first begin marker to the first end marker after it.
/// `Ok(None)` means `text` carries no marker at all.
///
/// # Errors
///
/// [`PartialFenceError`] when `text` carries a begin marker or an end marker without its counterpart.
pub fn scan_fence(text: &str) -> Result<Option<Fence<'_>>, PartialFenceError> {
    let Some(begin) = find_marker(text, 0, BEGIN_MARKER_PREFIX) else {
        if text.contains(END_MARKER_PREFIX) {
            return Err(PartialFenceError::LoneEndMarker);
        }
        if text.contains(BEGIN_MARKER_PREFIX) {
            return Err(PartialFenceError::LoneBeginMarker);
        }
        return Ok(None);
    };
    let Some(end) = find_marker(text, begin.range.end, END_MARKER_PREFIX) else {
        return Err(PartialFenceError::LoneBeginMarker);
    };
    let range = begin.range.start..end.range.end;
    Ok(Some(Fence {
        region: &text[range.clone()],
        range,
        version: begin.version,
    }))
}
