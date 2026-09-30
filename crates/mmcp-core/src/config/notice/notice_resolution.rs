//! [`NoticeResolution`], the effective value of a key with the layer that decided it.

use super::{NoticeLayers, NoticeSource, NoticeValue};

/// Result of resolving the [`NoticeLayers`] of one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoticeResolution {
    /// Effective value.
    pub effective: NoticeValue,
    /// Layer the effective value comes from.
    pub source: NoticeSource,
    /// Every layer's own value, for display beside the effective one.
    pub layers: NoticeLayers,
}
