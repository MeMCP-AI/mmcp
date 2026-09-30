//! [`ClaudeMdResolution`], the effective suggestion with the layer that decided it.

use super::{ClaudeMdLayers, ClaudeMdSource, ClaudeMdSuggestion};

/// Result of resolving [`ClaudeMdLayers`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeMdResolution {
    /// Effective suggestion.
    pub effective: ClaudeMdSuggestion,
    /// Layer the effective suggestion comes from.
    pub source: ClaudeMdSource,
    /// Every layer's own value, for display beside the effective one.
    pub layers: ClaudeMdLayers,
}
