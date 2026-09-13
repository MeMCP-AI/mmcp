//! Per-argument risk hint and its registry accessor.

use super::registry::tool_metadata_for_name;

/// Per-argument risk hint.
///
/// Each entry names a specific arg (and the value that activates the risk).
/// Harnesses can prompt even when the tool itself is not flagged destructive at the tool-annotation level.
/// Serialised into `describe_tools` and the `mmcp tools` CLI.
///
/// Today only boolean-true triggers are modelled: the existing risky args (`override`, `force`) are all flag-shaped.
/// Enum or numeric value triggers can extend the `risk_when` field later without breaking the wire shape.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ArgRiskHint {
    /// Name of the argument as it appears in the tool's input schema.
    pub arg: &'static str,
    /// Value condition that makes the arg risky.
    /// Today always `"true"` since every existing risky arg is boolean.
    pub risk_when: &'static str,
    /// Stable code matching the tool-level `destructive_hint` vocabulary so harnesses can re-use the same prompt text.
    pub kind: &'static str,
    /// Human-readable one-line explanation.
    /// Suitable for direct display in a confirmation prompt.
    pub reason: &'static str,
}

/// Curated per-argument risk hints for the live tool named
/// `tool_name`. Entries are hand-maintained in `registry::tool_metadata`:
/// there is no derive macro that inspects the args struct, and most
/// tool args are not risk-bearing, so the registry stays short.
pub(crate) fn arg_risk_hints_for(tool_name: &str) -> &'static [ArgRiskHint] {
    tool_metadata_for_name(tool_name).risk_hints
}
