//! Shared subscription plumbing.
//!
//! `commands::serve` (the `subscribe` / `unsubscribe` / `bootstrap_context`
//! MCP tools), `commands::subscribe` (the `mmcp subscribe` /
//! `mmcp unsubscribe` CLI subcommands), and `commands::bootstrap`
//! (the `mmcp bootstrap` CLI mirror) all need the same subscription
//! domain logic: validating and mutating a project's
//! `[subscriptions]` config (see [`target`]), and turning that
//! config into the concrete set of memory addresses it pulls into
//! scope (below). Both live here, their own concern-named module, so
//! no single caller owns them and every caller imports downward
//! instead of reaching into a peer command module.

mod target;

pub use target::{
    SubscribeError, SubscribeMcpArgs, SubscriptionAction, SubscriptionKind, apply_subscription,
    resolve_project_root, validate_subscription_target,
};

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use mmcp_store::diagnostics::Finding;
use mmcp_store::groups::GroupEntry;
use serde_json::json;
use uuid::Uuid;

use crate::notes::finding_to_note;

/// Resolve `bootstrap_context.next_action.subscribed_reads` from the
/// four subscription axes:
///
/// - `groups` + `languages` → every file in a fully-subscribed group
///   (Global scope, the caller's own Project scope, or an adopted
///   Shared group) collapses to ONE group-summary entry (`kind:
///   "group"`) instead of one entry per file, to keep
///   `bootstrap_context`'s response bounded on a group with many
///   memories. The per-file address list is strictly less
///   informative than the `list_memories(group)` walk the protocol
///   already mandates for every `groups_in_scope` entry, so emitting
///   one row per file was pure duplication (measured 213 -> 2
///   entries on this project's own mmcp group). The summary carries
///   a `count` and a `fetch_hint` naming the follow-up call.
/// - `memories` → literal `<group_uuid>:<slug>` pins. ALWAYS surface
///   as their own per-memory entry (`kind: "memory"`), even inside an
///   otherwise fully-subscribed group: an explicit pin
///   is real, non-derivable information the collapse must not absorb.
/// - `tags` → scan every group the local mirror knows about (not
///   only in-scope ones, since the whole point of tag pins is to reach
///   memories from groups the project hasn't fully adopted) and
///   include any non-mandatory memory whose tags overlap, as its own
///   per-memory entry. A tag match landing inside an already
///   fully-subscribed group is resolved INSIDE that group's own pass
///   below (never falls through to the generic tag-matching walk),
///   so that carve-out never duplicates work or re-inflates
///   the response with redundant entries for files the group summary
///   already covers.
///
/// Every entry in the returned array carries an explicit `kind`
/// discriminant (`"group"` or `"memory"`) so a caller can never
/// mistake one shape for the other.
///
/// A read/parse failure on any file (git read, non-UTF8 body,
/// frontmatter parse) or a failed group listing never silently
/// drops the address.
/// Each cause surfaces on the returned notes list
/// under its own code: `group_listing_failed` for a failed group
/// listing, `memory_read_failed` for a git read failure,
/// `memory_not_utf8` for a non-UTF8 body, and `frontmatter_parse_failed`
/// (the same code and shape `read_memory_descriptor` uses, via the
/// shared `parse_failed_finding` producer) only for an actual
/// frontmatter parse failure. No whole-group drop, no whole-file
/// drop; every sibling still lists/collapses normally.
///
/// Deduplicated across axes so a tag- or memory-pinned entry never
/// appears twice.
pub async fn resolve_subscribed_reads(
    backend: &NativeBackend,
    entries: &[GroupEntry],
    cfg: &mmcp_core::config::ProjectConfig,
    adopted_shared: &std::collections::HashSet<Uuid>,
    project_uuid: Option<Uuid>,
) -> (Vec<serde_json::Value>, Vec<mmcp_proto::Note>) {
    let mut seen: std::collections::HashSet<(Uuid, String)> = std::collections::HashSet::new();
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut notes: Vec<mmcp_proto::Note> = Vec::new();

    let want_tags: std::collections::HashSet<String> =
        cfg.subscriptions.tags.iter().cloned().collect();
    let want_memories: std::collections::HashSet<String> =
        cfg.subscriptions.memories.iter().cloned().collect();

    let push_memory_addr = |seen: &mut std::collections::HashSet<(Uuid, String)>,
                            out: &mut Vec<serde_json::Value>,
                            group: Uuid,
                            slug: &str| {
        if seen.insert((group, slug.to_string())) {
            out.push(json!({
                "kind": "memory",
                "group": group.to_string(),
                "slug": slug,
            }));
        }
    };

    for entry in entries {
        let entry_uuid = *entry.manifest.group_id.as_uuid();
        let fully_subscribed = match entry.manifest.scope {
            mmcp_core::manifest::GroupScope::Global => true,
            mmcp_core::manifest::GroupScope::Project => project_uuid == Some(entry_uuid),
            mmcp_core::manifest::GroupScope::Shared => adopted_shared.contains(&entry_uuid),
        };

        // Memory pins target a specific (group, slug); we always
        // need to walk every group's file list so the resolver
        // surfaces pins from groups that aren't fully subscribed.
        let need_listing = fully_subscribed || !want_tags.is_empty() || !want_memories.is_empty();
        if !need_listing {
            continue;
        }

        let files =
            match mmcp_store::list_all_memory_files(backend, &entry.handle, &Rev::head()).await {
                Ok(f) => f,
                Err(err) => {
                    // A group listing failure surfaces as
                    // `group_listing_failed` rather than dropping the
                    // group's entire address contribution silently:
                    // the group-level sibling of the per-file note below.
                    notes.push(finding_to_note(&Finding {
                        group: entry_uuid.to_string(),
                        slug: None,
                        severity: "error",
                        code: "group_listing_failed",
                        message: format!("failed to list memory files: {err}"),
                    }));
                    continue;
                }
            };

        if fully_subscribed {
            // An explicit pin (memory or tag) always wins over the
            // group collapse, so a deliberately-subscribed memory or
            // tag match keeps its own per-memory entry instead of
            // being absorbed into the one group-summary row below.
            // These files never fall through to the generic
            // tag-matching walk further down, since that walk is for
            // groups NOT already fully subscribed.
            let mut collapsed_count = 0usize;
            for file_ref in &files {
                let pin_key = format!("{entry_uuid}:{}", file_ref.slug);
                if want_memories.contains(&pin_key) {
                    push_memory_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                    continue;
                }

                // Read + parse once per file so a broken
                // frontmatter file is reported instead of silently
                // vanishing from the group's count, and so an
                // explicit tag pin inside an already fully-subscribed
                // group still earns its own entry without a second,
                // redundant walk over the same files.
                let file =
                    match read_memory_file_for_subscription(backend, entry, entry_uuid, file_ref)
                        .await
                    {
                        Ok(file) => file,
                        Err(note) => {
                            notes.push(note);
                            continue;
                        }
                    };

                if !file.frontmatter.mandatory
                    && !want_tags.is_empty()
                    && file.frontmatter.tags.iter().any(|t| want_tags.contains(t))
                {
                    push_memory_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                    continue;
                }

                collapsed_count += 1;
            }
            if collapsed_count > 0 {
                out.push(json!({
                    "kind": "group",
                    "group": entry_uuid.to_string(),
                    "slug": entry.manifest.slug,
                    "scope": match entry.manifest.scope {
                        mmcp_core::manifest::GroupScope::Global => "global",
                        mmcp_core::manifest::GroupScope::Shared => "shared",
                        mmcp_core::manifest::GroupScope::Project => "project",
                    },
                    "count": collapsed_count,
                    "fetch_hint": format!("list_memories(group={entry_uuid})"),
                }));
            }
            continue;
        }

        for file_ref in &files {
            let pin_key = format!("{entry_uuid}:{}", file_ref.slug);
            let pinned_individually = want_memories.contains(&pin_key);

            if pinned_individually {
                push_memory_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                continue;
            }
            if !want_tags.is_empty() {
                let file =
                    match read_memory_file_for_subscription(backend, entry, entry_uuid, file_ref)
                        .await
                    {
                        Ok(file) => file,
                        Err(note) => {
                            notes.push(note);
                            continue;
                        }
                    };
                if file.frontmatter.mandatory {
                    // Tag-based subscription is intentionally about
                    // non-mandatory memories; mandatory entries
                    // already surface through `list_memories` on
                    // every in-scope group.
                    continue;
                }
                if file.frontmatter.tags.iter().any(|t| want_tags.contains(t)) {
                    push_memory_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                }
            }
        }
    }

    (out, notes)
}

/// Read and parse one memory file inside `resolve_subscribed_reads`'s
/// pin/tag matching passes, converting every failure mode into its
/// own note code: `memory_read_failed` for a git read failure,
/// `memory_not_utf8` for a non-UTF8 body, and `frontmatter_parse_failed`
/// (the same shape `read_memory_descriptor` already established) for
/// an actual frontmatter parse failure. None of the three silently
/// drops the file anymore.
async fn read_memory_file_for_subscription(
    backend: &NativeBackend,
    entry: &GroupEntry,
    entry_uuid: Uuid,
    file_ref: &mmcp_store::MemoryFileRef,
) -> Result<MemoryFile, mmcp_proto::Note> {
    let bytes = backend
        .read_file(&entry.handle, &file_ref.path, &Rev::head())
        .await
        .map_err(|err| {
            finding_to_note(&Finding {
                group: entry_uuid.to_string(),
                slug: Some(file_ref.slug.clone()),
                severity: "error",
                code: "memory_read_failed",
                message: format!("failed to read memory file: {err}"),
            })
        })?;
    let text = std::str::from_utf8(&bytes).map_err(|err| {
        finding_to_note(&Finding {
            group: entry_uuid.to_string(),
            slug: Some(file_ref.slug.clone()),
            severity: "error",
            code: "memory_not_utf8",
            message: format!("memory file is not valid UTF-8: {err}"),
        })
    })?;
    MemoryFile::parse(text).map_err(|err| {
        finding_to_note(&mmcp_store::tracker::parse_failed_finding(
            &entry_uuid.to_string(),
            &file_ref.slug,
            &err,
        ))
    })
}
