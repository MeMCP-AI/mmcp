//! Shared per-tool metadata registry: icon category, `_meta`
//! advisory keys, and argument risk hints.
//!
//! `commands::serve` (both the live `#[tool_router]` patching inside
//! `McpServer::new` and the `describe_tools` MCP tool) and
//! `commands::tools` (the `mmcp tools` CLI, via `main.rs` as its
//! composition root) both decorate the same canonical tool list with
//! the same icons / `_meta` / risk hints. The registry is pure data
//! over [`McpToolId`] and `rmcp::model::*` types, with no dependency
//! on `McpServer` itself, so it lives here instead of in
//! `commands::serve`, and [`decorate_tool_attrs`] is the shared
//! decoration step both callers apply to their own base tool list.
//!
//! `McpServer::registered_tool_attrs()` (the live, macro-derived base list) stays in
//! `commands::serve`: it reads `Self::tool_router().map`, which only exists on `McpServer`'s own
//! `#[tool_router]` impl block.
//! See commands::tools's module doc for why this never imports from commands::serve.

use mmcp_proto::McpToolId;

mod defaults;

use defaults::{
    DEBUG_ICON_SRC, FEATURE_ICON_SRC, ISSUE_ICON_SRC, META_DEBUG_GATED, META_NETWORK,
    META_PROTECTED_GROUP_GATED, META_REQUIRES_PROJECT, META_REQUIRES_SYNC, MILESTONE_ICON_SRC,
    MUTATE_ICON_SRC, READ_ICON_SRC, SYNC_ICON_SRC,
};

/// Per-tool category that drives icon selection.
/// Declared per tool in [`tool_metadata`], the single exhaustive
/// registry backing icons, `_meta`, and argument risk hints alike;
/// there is no default arm, so a `#[tool]` method without a
/// [`McpToolId`] variant and a `tool_metadata` arm fails to compile
/// instead of shipping a generic glyph that misleads operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolIconCategory {
    /// Read-only tools that walk the local mirror without writing.
    Read,
    /// Local mutators: additive or destructive writes against the
    /// mirror, the sessions store, or `.mmcp.toml`.
    Mutate,
    /// Feature-request tools (`*_feature`).
    Feature,
    /// Issue-tracker tools (`*_issue`), sister to `Feature`.
    Issue,
    /// Milestone tracker tools (`*_milestone`), sister to `Feature`
    /// / `Issue` but a reduced surface.
    Milestone,
    /// `debug_*` raw-git escape hatches.
    Debug,
    /// `sync_*` tools that contact the remote server.
    Sync,
}

/// Per-tool metadata bundle: icon category, `_meta` advisory keys,
/// and argument risk hints, produced together by one
/// [`tool_metadata`] match arm.
#[derive(Clone, Copy)]
struct ToolMetadata {
    category: ToolIconCategory,
    /// Namespaced `_meta` keys this tool sets to `true`; empty means
    /// no advisory bits, so the wire `_meta` stays absent.
    meta_keys: &'static [&'static str],
    risk_hints: &'static [ArgRiskHint],
}

/// The single owning per-tool metadata registry.
///
/// One exhaustive match over [`McpToolId`], with no wildcard arm:
/// every registered `#[tool]` method needs a variant here (added in
/// the same change as the method itself) and a match arm declaring
/// its icon category, `_meta` keys, and argument risk hints
/// together.
///
/// `_meta` vocabulary:
/// - `mmcp.requires_project`: tool errors without a discovered
///   `.mmcp.toml` (every FR tool plus `subscribe` / `unsubscribe`).
/// - `mmcp.requires_sync`: tool errors without a configured
///   `[sync]` block in `.mmcp.toml` (every `sync_*` tool).
/// - `mmcp.debug_gated`: tool refuses unless `debug_toggle(true)`
///   has been called this session (every `debug_*` tool).
/// - `mmcp.protected_group_gated`: tool fires the
///   `confirm_protected_write` elicitation when targeting a
///   protected group (write / edit / delete / debug_write_file /
///   the feature-tracker mutators / the issue-tracker mutators).
/// - `mmcp.network`: tool reaches outside the local mirror.
///   Today only the `sync_*` tools set this, mirroring
///   `open_world_hint` but kept distinct so future open-world
///   tools that don't sync (e.g. a future fetch-from-URL) classify
///   cleanly.
fn tool_metadata(id: McpToolId) -> ToolMetadata {
    match id {
        // Read-only tools.
        McpToolId::ListGroups
        | McpToolId::ListMemories
        | McpToolId::ReadMemory
        | McpToolId::ListVersions
        | McpToolId::GroupInfo
        | McpToolId::SearchMemories
        | McpToolId::ReadMemoryBodySections
        | McpToolId::CheckHealth
        | McpToolId::Diagnose
        | McpToolId::BootstrapContext
        | McpToolId::Status
        | McpToolId::Version
        | McpToolId::DescribeTools
        // Archive export reads the store to produce an artifact.
        | McpToolId::ExportArchive => ToolMetadata {
            category: ToolIconCategory::Read,
            meta_keys: &[],
            risk_hints: &[],
        },
        // Feature-tracker tools.
        McpToolId::ReadFeature | McpToolId::ListFeatures => ToolMetadata {
            category: ToolIconCategory::Feature,
            meta_keys: &[META_REQUIRES_PROJECT],
            risk_hints: &[],
        },
        McpToolId::AddFeature
        | McpToolId::UpdateFeature
        | McpToolId::DeleteFeature
        | McpToolId::RenameFeature => ToolMetadata {
            category: ToolIconCategory::Feature,
            meta_keys: &[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED],
            risk_hints: &[],
        },
        // Issue-tracker tools, sister to Feature.
        McpToolId::ReadIssue | McpToolId::ListIssues => ToolMetadata {
            category: ToolIconCategory::Issue,
            meta_keys: &[META_REQUIRES_PROJECT],
            risk_hints: &[],
        },
        McpToolId::AddIssue
        | McpToolId::UpdateIssue
        | McpToolId::DeleteIssue
        | McpToolId::RenameIssue => ToolMetadata {
            category: ToolIconCategory::Issue,
            meta_keys: &[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED],
            risk_hints: &[],
        },
        // Milestone-tracker tools, sister to Feature / Issue but a
        // reduced surface (no rename/delete tool exists yet).
        McpToolId::ReadMilestone | McpToolId::ListMilestones => ToolMetadata {
            category: ToolIconCategory::Milestone,
            meta_keys: &[META_REQUIRES_PROJECT],
            risk_hints: &[],
        },
        McpToolId::AddMilestone | McpToolId::UpdateMilestone => ToolMetadata {
            category: ToolIconCategory::Milestone,
            meta_keys: &[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED],
            risk_hints: &[],
        },
        // `debug_*` raw-git escape hatches.
        McpToolId::DebugReadFile | McpToolId::DebugListTree | McpToolId::DebugGitLog => {
            ToolMetadata {
                category: ToolIconCategory::Debug,
                meta_keys: &[META_DEBUG_GATED],
                risk_hints: &[],
            }
        }
        McpToolId::DebugToggle => ToolMetadata {
            category: ToolIconCategory::Debug,
            meta_keys: &[META_DEBUG_GATED],
            risk_hints: &[],
        },
        McpToolId::DebugWriteFile => ToolMetadata {
            category: ToolIconCategory::Debug,
            meta_keys: &[META_DEBUG_GATED, META_PROTECTED_GROUP_GATED],
            risk_hints: &[],
        },
        // `sync_*` tools that contact the remote server.
        McpToolId::SyncFetch | McpToolId::SyncPush | McpToolId::SyncPull | McpToolId::Sync => {
            ToolMetadata {
                category: ToolIconCategory::Sync,
                meta_keys: &[META_REQUIRES_SYNC, META_NETWORK],
                risk_hints: &[],
            }
        }
        // Local mutators with no advisory bits or risk hints.
        McpToolId::ImportMemory => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[],
            risk_hints: &[ArgRiskHint {
                arg: "override",
                risk_when: "true",
                kind: "destructive",
                reason: "override: true replaces the colliding-id memory in place",
            }],
        },
        McpToolId::MoveMemory | McpToolId::DeleteMemory => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_PROTECTED_GROUP_GATED],
            risk_hints: &[],
        },
        McpToolId::InitClaude | McpToolId::InitProject | McpToolId::CreateGroup => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[],
            risk_hints: &[],
        },
        McpToolId::Subscribe | McpToolId::Unsubscribe => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_REQUIRES_PROJECT],
            risk_hints: &[],
        },
        McpToolId::WriteMemory => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_PROTECTED_GROUP_GATED],
            risk_hints: &[
                ArgRiskHint {
                    arg: "override",
                    risk_when: "true",
                    kind: "destructive",
                    reason: "override: true overwrites the existing file silently; prefer edit_memory for partial updates",
                },
                ArgRiskHint {
                    arg: "force",
                    risk_when: "true",
                    kind: "destructive",
                    reason: "force: true bypasses the filename/frontmatter id-mismatch guard",
                },
            ],
        },
        McpToolId::EditMemory => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_PROTECTED_GROUP_GATED],
            risk_hints: &[ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the filename/frontmatter id-mismatch guard on a ByFilename write",
            }],
        },
        McpToolId::EditMemoryBody => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_PROTECTED_GROUP_GATED],
            risk_hints: &[ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the filename/frontmatter id-mismatch guard",
            }],
        },
        McpToolId::ImportArchive => ToolMetadata {
            category: ToolIconCategory::Mutate,
            meta_keys: &[META_PROTECTED_GROUP_GATED],
            risk_hints: &[ArgRiskHint {
                arg: "overwrite",
                risk_when: "true",
                kind: "destructive",
                reason: "overwrite: true replaces colliding memories in place instead of reporting a conflict",
            }],
        },
    }
}

/// Resolve a live tool's metadata bundle from its wire name.
///
/// # Panics
/// Panics when `name` has no [`McpToolId`]: every `#[tool]`-registered
/// method must gain a matching variant, a [`McpToolId::parse`] arm, and a
/// [`tool_metadata`] arm in the same change, so reaching here signals a
/// registration gap rather than a normal runtime condition.
fn tool_metadata_for_name(name: &str) -> ToolMetadata {
    let id = McpToolId::parse(name).unwrap_or_else(|| {
        panic!(
            "'{name}' has no McpToolId variant; add one, a McpToolId::parse arm, and a \
             tool_metadata arm before registering its #[tool] method",
        )
    });
    tool_metadata(id)
}

pub(crate) fn tool_icon_category(name: &str) -> ToolIconCategory {
    tool_metadata_for_name(name).category
}

pub(crate) fn icons_for_category(cat: ToolIconCategory) -> Vec<rmcp::model::Icon> {
    let src = match cat {
        ToolIconCategory::Read => READ_ICON_SRC,
        ToolIconCategory::Mutate => MUTATE_ICON_SRC,
        ToolIconCategory::Feature => FEATURE_ICON_SRC,
        ToolIconCategory::Issue => ISSUE_ICON_SRC,
        ToolIconCategory::Milestone => MILESTONE_ICON_SRC,
        ToolIconCategory::Debug => DEBUG_ICON_SRC,
        ToolIconCategory::Sync => SYNC_ICON_SRC,
    };
    vec![rmcp::model::Icon::new(src).with_mime_type("image/svg+xml")]
}

/// Build the per-tool `_meta` map carrying mmcp-specific advisory
/// hints that complement the `ToolAnnotations` bits.
/// Returns `None` for tools that need none of the bits so the wire
/// shape stays absent rather than `{}` for unrelated tools.
/// Vocabulary and per-tool assignment live in [`tool_metadata`].
pub(crate) fn meta_for_tool(name: &str) -> Option<rmcp::model::MetaObject> {
    let keys = tool_metadata_for_name(name).meta_keys;
    if keys.is_empty() {
        return None;
    }
    let mut meta = rmcp::model::MetaObject::new();
    for k in keys {
        meta.0
            .insert((*k).to_string(), serde_json::Value::Bool(true));
    }
    Some(meta)
}

/// Every registered tool receives a permissive object
/// `output_schema` so MCP clients can validate that responses are
/// JSON objects (with optional `notes` channel) and surface the
/// shape in autocomplete UIs.
///
/// Cached behind a `OnceLock` so the same `Arc<JsonObject>` reaches
/// every tool. Cheap to clone; cheaper than rebuilding the map per
/// tool on every `tools/list` round-trip.
pub(crate) fn shared_output_schema() -> std::sync::Arc<rmcp::model::JsonObject> {
    use std::sync::OnceLock;
    static SCHEMA: OnceLock<std::sync::Arc<rmcp::model::JsonObject>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("object".to_string()),
            );
            obj.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(true),
            );
            // Surface the shared `notes` field shape so harnesses
            // know to look there for dangling-ref / parse-warning
            // notes; absent on tools that never emit any.
            let mut props = serde_json::Map::new();
            let mut notes_schema = serde_json::Map::new();
            notes_schema.insert(
                "type".to_string(),
                serde_json::Value::String("array".to_string()),
            );
            notes_schema.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "Standard notes channel. Optional warnings emitted alongside the \
                     tool's primary payload."
                        .to_string(),
                ),
            );
            props.insert("notes".to_string(), serde_json::Value::Object(notes_schema));
            obj.insert("properties".to_string(), serde_json::Value::Object(props));
            obj.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "Tool response. Permissive object shape; per-tool typed schemas \
                     are a candidate future refinement."
                        .to_string(),
                ),
            );
            std::sync::Arc::new(obj)
        })
        .clone()
}

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
/// `tool_name`. Entries are hand-maintained in [`tool_metadata`]:
/// there is no derive macro that inspects the args struct, and most
/// tool args are not risk-bearing, so the registry stays short.
pub(crate) fn arg_risk_hints_for(tool_name: &str) -> &'static [ArgRiskHint] {
    tool_metadata_for_name(tool_name).risk_hints
}

/// Decorate a base tool list with icons, `_meta`, and output schema.
/// Takes the base list as a parameter rather than fetching it itself.
/// See commands::tools's module doc for why this never imports from commands::serve.
pub(crate) fn decorate_tool_attrs(mut tools: Vec<rmcp::model::Tool>) -> Vec<rmcp::model::Tool> {
    for tool in &mut tools {
        tool.icons = Some(icons_for_category(tool_icon_category(tool.name.as_ref())));
        // Meta lands on the same patching seam as icons so
        // describe_tools and the CLI surface match the live router.
        tool.meta = meta_for_tool(tool.name.as_ref());
        // Every tool gets a permissive object output schema
        // so clients can validate.
        // Per-tool typed schemas are a candidate future refinement.
        tool.output_schema = Some(shared_output_schema());
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot-check that category routing covers the obvious
    /// archetypes: one read tool, one debug tool, one sync tool,
    /// one feature tool, one mutate tool, so a future refactor of
    /// the category match can't silently re-bucket entire families.
    #[test]
    fn tool_icon_category_covers_each_archetype() {
        assert_eq!(tool_icon_category("read_memory"), ToolIconCategory::Read);
        assert_eq!(tool_icon_category("write_memory"), ToolIconCategory::Mutate);
        assert_eq!(
            tool_icon_category("read_feature"),
            ToolIconCategory::Feature
        );
        assert_eq!(
            tool_icon_category("debug_read_file"),
            ToolIconCategory::Debug,
        );
        assert_eq!(tool_icon_category("sync_pull"), ToolIconCategory::Sync);
    }

    /// An unmapped tool name panics.
    /// This is the completeness guarantee `tool_metadata` exists for:
    /// a `#[tool]` method that ships without a matching `McpToolId`
    /// variant fails loudly here rather than shipping a misleading icon.
    #[test]
    #[should_panic(expected = "has no McpToolId variant")]
    fn tool_icon_category_rejects_an_unmapped_name() {
        tool_icon_category("future_tool_that_does_not_exist_yet");
    }

    /// The patching seam decorates each tool with its
    /// mmcp.* advisory bits.
    /// Spot-check the four buckets: sync (network + requires_sync),
    /// debug (debug_gated), protected-group (write_memory hits the
    /// protected-group guard), feature (requires_project), so a
    /// refactor of `meta_for_tool` cannot silently strip the
    /// wire-visible hints.
    #[test]
    fn meta_for_tool_covers_each_namespace_bucket() {
        let sync = meta_for_tool("sync_pull").expect("sync_pull has meta");
        assert_eq!(
            sync.0.get("mmcp.requires_sync"),
            Some(&serde_json::Value::Bool(true)),
        );
        assert_eq!(
            sync.0.get("mmcp.network"),
            Some(&serde_json::Value::Bool(true)),
        );

        let debug = meta_for_tool("debug_read_file").expect("debug has meta");
        assert_eq!(
            debug.0.get("mmcp.debug_gated"),
            Some(&serde_json::Value::Bool(true)),
        );

        let protected = meta_for_tool("write_memory").expect("write_memory has meta");
        assert_eq!(
            protected.0.get("mmcp.protected_group_gated"),
            Some(&serde_json::Value::Bool(true)),
        );

        let feature = meta_for_tool("read_feature").expect("read_feature has meta");
        assert_eq!(
            feature.0.get("mmcp.requires_project"),
            Some(&serde_json::Value::Bool(true)),
        );

        // A tool with no advisory bits returns `None` so the wire
        // stays absent, not `{}`. `list_groups` is a pure-local
        // read with no preconditions.
        assert!(meta_for_tool("list_groups").is_none());
    }

    /// The schema is shared (same `Arc`) across all tools so
    /// the per-tool patch is cheap.
    /// Cloning the Arc bumps the reference count rather than
    /// rebuilding the JsonObject.
    #[test]
    fn shared_output_schema_returns_same_arc() {
        let a = shared_output_schema();
        let b = shared_output_schema();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "shared_output_schema must hand out the same Arc on repeat calls",
        );
    }
}
