//! The single owning per-tool metadata registry.

use mmcp_proto::McpToolId;

use super::arg_risk_hint::ArgRiskHint;
use super::defaults::{
    CLAUDE_CODE_MAX_RESULT_SIZE_CHARS, META_ANTHROPIC_MAX_RESULT_SIZE_CHARS, META_DEBUG_GATED,
    META_NETWORK, META_PROTECTED_GROUP_GATED, META_REQUIRES_PROJECT, META_REQUIRES_SYNC,
};
use super::icon_category::ToolIconCategory;

/// Per-tool metadata bundle: icon category, `_meta` advisory keys,
/// and argument risk hints, produced together by one
/// [`tool_metadata`] match arm.
#[derive(Clone, Copy)]
pub(super) struct ToolMetadata {
    pub(super) category: ToolIconCategory,
    /// Namespaced `_meta` keys this tool sets to `true`; empty means
    /// no advisory bits, so the wire `_meta` stays absent.
    pub(super) meta_keys: &'static [&'static str],
    /// Value for the numeric `anthropic/maxResultSizeChars` `_meta` key; `None` omits it.
    pub(super) max_result_size_chars: Option<u64>,
    pub(super) risk_hints: &'static [ArgRiskHint],
}

/// Builder for [`ToolMetadata`].
///
/// Defaults `meta_keys` and `risk_hints` to empty, so a
/// `tool_metadata` arm with no advisory bits and no risk hints
/// names only its category.
struct ToolMetadataBuilder {
    category: ToolIconCategory,
    meta_keys: &'static [&'static str],
    max_result_size_chars: Option<u64>,
    risk_hints: &'static [ArgRiskHint],
}

impl ToolMetadataBuilder {
    fn new(category: ToolIconCategory) -> Self {
        Self {
            category,
            meta_keys: &[],
            max_result_size_chars: None,
            risk_hints: &[],
        }
    }

    fn meta_keys(mut self, meta_keys: &'static [&'static str]) -> Self {
        self.meta_keys = meta_keys;
        self
    }

    fn max_result_size_chars(mut self, chars: u64) -> Self {
        self.max_result_size_chars = Some(chars);
        self
    }

    fn risk_hints(mut self, risk_hints: &'static [ArgRiskHint]) -> Self {
        self.risk_hints = risk_hints;
        self
    }

    fn build(self) -> ToolMetadata {
        ToolMetadata {
            category: self.category,
            meta_keys: self.meta_keys,
            max_result_size_chars: self.max_result_size_chars,
            risk_hints: self.risk_hints,
        }
    }
}

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
        // `read_memory` returns a memory body whole, with no truncation.
        // Declares the calling client's own inline-result ceiling, so a client honoring the key never side-files it.
        McpToolId::ReadMemory => ToolMetadataBuilder::new(ToolIconCategory::Read)
            .max_result_size_chars(CLAUDE_CODE_MAX_RESULT_SIZE_CHARS)
            .build(),
        // Read-only tools.
        McpToolId::ListGroups
        | McpToolId::ListMemories
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
        | McpToolId::ExportArchive => ToolMetadataBuilder::new(ToolIconCategory::Read).build(),
        // Feature-tracker tools.
        McpToolId::ReadFeature | McpToolId::ListFeatures => {
            ToolMetadataBuilder::new(ToolIconCategory::Feature)
                .meta_keys(&[META_REQUIRES_PROJECT])
                .build()
        }
        McpToolId::AddFeature
        | McpToolId::UpdateFeature
        | McpToolId::DeleteFeature
        | McpToolId::RenameFeature => ToolMetadataBuilder::new(ToolIconCategory::Feature)
            .meta_keys(&[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED])
            .build(),
        // Issue-tracker tools, sister to Feature.
        McpToolId::ReadIssue | McpToolId::ListIssues => {
            ToolMetadataBuilder::new(ToolIconCategory::Issue)
                .meta_keys(&[META_REQUIRES_PROJECT])
                .build()
        }
        McpToolId::AddIssue
        | McpToolId::UpdateIssue
        | McpToolId::DeleteIssue
        | McpToolId::RenameIssue => ToolMetadataBuilder::new(ToolIconCategory::Issue)
            .meta_keys(&[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED])
            .build(),
        // Milestone-tracker tools, sister to Feature / Issue but a
        // reduced surface (no rename/delete tool exists yet).
        McpToolId::ReadMilestone | McpToolId::ListMilestones => {
            ToolMetadataBuilder::new(ToolIconCategory::Milestone)
                .meta_keys(&[META_REQUIRES_PROJECT])
                .build()
        }
        McpToolId::AddMilestone | McpToolId::UpdateMilestone => {
            ToolMetadataBuilder::new(ToolIconCategory::Milestone)
                .meta_keys(&[META_REQUIRES_PROJECT, META_PROTECTED_GROUP_GATED])
                .build()
        }
        // `debug_*` raw-git escape hatches.
        McpToolId::DebugReadFile | McpToolId::DebugListTree | McpToolId::DebugGitLog => {
            ToolMetadataBuilder::new(ToolIconCategory::Debug)
                .meta_keys(&[META_DEBUG_GATED])
                .build()
        }
        McpToolId::DebugToggle => ToolMetadataBuilder::new(ToolIconCategory::Debug)
            .meta_keys(&[META_DEBUG_GATED])
            .build(),
        McpToolId::DebugWriteFile => ToolMetadataBuilder::new(ToolIconCategory::Debug)
            .meta_keys(&[META_DEBUG_GATED, META_PROTECTED_GROUP_GATED])
            .build(),
        // `sync_*` tools that contact the remote server.
        McpToolId::SyncFetch | McpToolId::SyncPush | McpToolId::SyncPull | McpToolId::Sync => {
            ToolMetadataBuilder::new(ToolIconCategory::Sync)
                .meta_keys(&[META_REQUIRES_SYNC, META_NETWORK])
                .build()
        }
        // Local mutators with no advisory bits or risk hints.
        McpToolId::ImportMemory => ToolMetadataBuilder::new(ToolIconCategory::Mutate)
            .risk_hints(&[ArgRiskHint {
                arg: "override",
                risk_when: "true",
                kind: "destructive",
                reason: "override: true replaces the colliding-id memory in place",
            }])
            .build(),
        McpToolId::MoveMemory | McpToolId::DeleteMemory => {
            ToolMetadataBuilder::new(ToolIconCategory::Mutate)
                .meta_keys(&[META_PROTECTED_GROUP_GATED])
                .build()
        }
        McpToolId::InitClaude | McpToolId::InitProject | McpToolId::CreateGroup => {
            ToolMetadataBuilder::new(ToolIconCategory::Mutate).build()
        }
        McpToolId::Subscribe | McpToolId::Unsubscribe => {
            ToolMetadataBuilder::new(ToolIconCategory::Mutate)
                .meta_keys(&[META_REQUIRES_PROJECT])
                .build()
        }
        McpToolId::WriteMemory => ToolMetadataBuilder::new(ToolIconCategory::Mutate)
            .meta_keys(&[META_PROTECTED_GROUP_GATED])
            .risk_hints(&[
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
            ])
            .build(),
        McpToolId::EditMemory => ToolMetadataBuilder::new(ToolIconCategory::Mutate)
            .meta_keys(&[META_PROTECTED_GROUP_GATED])
            .risk_hints(&[ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the filename/frontmatter id-mismatch guard on a ByFilename write",
            }])
            .build(),
        McpToolId::EditMemoryBody => ToolMetadataBuilder::new(ToolIconCategory::Mutate)
            .meta_keys(&[META_PROTECTED_GROUP_GATED])
            .risk_hints(&[ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the filename/frontmatter id-mismatch guard",
            }])
            .build(),
        McpToolId::ImportArchive => ToolMetadataBuilder::new(ToolIconCategory::Mutate)
            .meta_keys(&[META_PROTECTED_GROUP_GATED])
            .risk_hints(&[ArgRiskHint {
                arg: "overwrite",
                risk_when: "true",
                kind: "destructive",
                reason: "overwrite: true replaces colliding memories in place instead of reporting a conflict",
            }])
            .build(),
    }
}

/// Resolve a live tool's metadata bundle from its wire name.
///
/// # Panics
/// Panics when `name` has no [`McpToolId`]: every `#[tool]`-registered
/// method must gain a matching variant, a [`McpToolId::parse`] arm, and a
/// [`tool_metadata`] arm in the same change, so reaching here signals a
/// registration gap rather than a normal runtime condition.
pub(super) fn tool_metadata_for_name(name: &str) -> ToolMetadata {
    let id = McpToolId::parse(name).unwrap_or_else(|| {
        panic!(
            "'{name}' has no McpToolId variant; add one, a McpToolId::parse arm, and a \
             tool_metadata arm before registering its #[tool] method",
        )
    });
    tool_metadata(id)
}

/// Build the per-tool `_meta` map carrying mmcp-specific advisory
/// hints that complement the `ToolAnnotations` bits.
/// Returns `None` for tools that need none of the bits so the wire
/// shape stays absent rather than `{}` for unrelated tools.
/// Vocabulary and per-tool assignment live in [`tool_metadata`].
pub(crate) fn meta_for_tool(name: &str) -> Option<rmcp::model::MetaObject> {
    let metadata = tool_metadata_for_name(name);
    if metadata.meta_keys.is_empty() && metadata.max_result_size_chars.is_none() {
        return None;
    }
    let mut meta = rmcp::model::MetaObject::new();
    for k in metadata.meta_keys {
        meta.0
            .insert((*k).to_string(), serde_json::Value::Bool(true));
    }
    if let Some(chars) = metadata.max_result_size_chars {
        meta.0.insert(
            META_ANTHROPIC_MAX_RESULT_SIZE_CHARS.to_string(),
            serde_json::Value::Number(chars.into()),
        );
    }
    Some(meta)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

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

    #[test]
    fn read_memory_declares_the_anthropic_max_result_size_chars_key() {
        let meta = meta_for_tool("read_memory").expect("read_memory has meta");
        assert_eq!(
            meta.0.get("anthropic/maxResultSizeChars"),
            Some(&serde_json::Value::Number(
                CLAUDE_CODE_MAX_RESULT_SIZE_CHARS.into()
            )),
        );
    }
}
