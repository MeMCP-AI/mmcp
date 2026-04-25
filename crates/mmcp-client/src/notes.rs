//! FR-45 notes-channel helpers shared by the MCP stdio server and
//! the CLI subcommands. Both surfaces render the same
//! [`mmcp_proto::Note`] vocabulary so a note code an operator sees
//! on the command line matches what an AI caller sees through MCP.
//!
//! The MCP path wraps responses with `ok_json_with_notes` (see
//! `commands::serve`); the CLI path calls [`render_notes_tail`] at
//! the foot of each subcommand's output. Mapping store-side
//! diagnostic issues onto notes is handled by [`issues_to_notes`]
//! so the two renderers stay in lockstep.
//!
//! The module also owns the [`dangling_ref_notes_for`] and
//! [`malformed_frontmatter_notes`] populators that were previously
//! inlined in `commands::serve`. Exposing them here lets every
//! `mmcp feature *` / `mmcp memory read` CLI entry point and the
//! corresponding MCP tool thread the same detection logic without
//! duplicating code.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{NativeBackend, Rev};
use mmcp_proto::{Note, NoteLevel};
use mmcp_store::diagnostics::Issue;
use mmcp_store::features::FeatureRecord;
use mmcp_store::groups::GroupEntry;
use mmcp_store::memory::list_all_memory_files;
use mmcp_sync::PushReport;
use serde_json::json;
use uuid::Uuid;

/// Print every note on its own line, prefixed by its severity.
///
/// Called at the tail of every CLI subcommand that has notes to
/// surface. Silent when the slice is empty so commands without
/// anomalies stay quiet. The leading blank line separates the
/// structured output above from the notes tail.
pub fn render_notes_tail(notes: &[Note]) {
    if notes.is_empty() {
        return;
    }
    println!();
    for note in notes {
        let prefix = match note.level {
            NoteLevel::Info => "info",
            NoteLevel::Warn => "warn",
            NoteLevel::Error => "error",
        };
        println!("{prefix}: [{}] {}", note.code, note.message);
    }
}

/// Convert a store-side [`Issue`] into a [`Note`] on the FR-45 wire.
///
/// `severity` maps to `level`; `code` flows through unchanged;
/// `group` + optional `slug` ride in `context` so callers can
/// route or group notes without re-parsing the message.
#[must_use]
pub fn issue_to_note(issue: &Issue) -> Note {
    let level = match issue.severity {
        "error" => NoteLevel::Error,
        "warning" => NoteLevel::Warn,
        _ => NoteLevel::Info,
    };
    let mut ctx = serde_json::Map::new();
    ctx.insert("group".to_string(), json!(issue.group));
    if let Some(ref slug) = issue.slug {
        ctx.insert("slug".to_string(), json!(slug));
    }
    Note {
        level,
        code: issue.code.to_string(),
        message: issue.message.clone(),
        context: Some(serde_json::Value::Object(ctx)),
    }
}

/// Map a slice of diagnostic issues onto the FR-45 notes channel.
#[must_use]
pub fn issues_to_notes(issues: &[Issue]) -> Vec<Note> {
    issues.iter().map(issue_to_note).collect()
}

/// FR-45 populator helper: scan a feature record's typed UUID
/// references (`depends_on`, `blocks`, `superseded_by.target`)
/// against the group's local memory index and emit a
/// `dangling_ref` note for each target that does not resolve.
///
/// Pure-local check — only verifies the uuid appears as a filename
/// under `memories/<slug>/<uuid>.md` somewhere in the group.
/// Cross-group refs always show up as dangling here until the
/// resolver learns to look across mirrored groups (separate FR).
pub async fn dangling_ref_notes_for(
    backend: &NativeBackend,
    entry: &GroupEntry,
    record: &FeatureRecord,
) -> Vec<Note> {
    let files = match list_all_memory_files(backend, &entry.handle, &Rev::head()).await {
        Ok(files) => files,
        // Git failure on enumeration is unusual; surface nothing
        // rather than inventing a fake dangling-ref storm. The
        // per-memory reads in the tool body would have failed too
        // and landed as a real error response upstream.
        Err(_) => return Vec::new(),
    };
    let known: std::collections::HashSet<Uuid> = files.iter().map(|f| f.id).collect();

    let mut notes = Vec::new();
    for (field, uuid) in record
        .depends_on
        .iter()
        .map(|u| ("depends_on", *u))
        .chain(record.blocks.iter().map(|u| ("blocks", *u)))
    {
        if !known.contains(&uuid) {
            notes.push(
                Note::warn(
                    "dangling_ref",
                    format!(
                        "feature `{}` references unresolved target in `{field}`",
                        record.slug
                    ),
                )
                .with_context(json!({
                    "slug": record.slug,
                    "field": field,
                    "target": uuid.to_string(),
                    "group": entry.manifest.group_id.to_string(),
                })),
            );
        }
    }
    if let Some(link) = record.superseded_by.as_ref() {
        if !known.contains(&link.target) {
            notes.push(
                Note::warn(
                    "dangling_ref",
                    format!(
                        "feature `{}` superseded_by target does not resolve locally",
                        record.slug
                    ),
                )
                .with_context(json!({
                    "slug": record.slug,
                    "field": "superseded_by",
                    "target": link.target.to_string(),
                    "commit": link.commit,
                    "group": entry.manifest.group_id.to_string(),
                })),
            );
        }
    }
    notes
}

/// FR-45 populator helper: scan a push report for groups whose
/// control-plane push succeeded but whose content plane (git
/// push) did not actually ship bytes. Each such group surfaces
/// as one `sync_partial_failure` warn so callers don't assume
/// silent success.
///
/// Shared by the MCP `sync_push` tool and the CLI `mmcp push` /
/// `mmcp sync` commands so both emit the same code with the
/// same context shape (`group`, `stage`, `server_url`).
#[must_use]
pub fn sync_push_partial_failure_notes(report: &PushReport, server_url: &str) -> Vec<Note> {
    report
        .pushed
        .iter()
        .filter(|p| !p.content_transferred)
        .map(|p| {
            Note::warn(
                "sync_partial_failure",
                format!(
                    "push for group {} did not ship content (transport error or unsupported backend)",
                    p.group_id
                ),
            )
            .with_context(json!({
                "group": p.group_id.to_string(),
                "stage": "push",
                "server_url": server_url,
            }))
        })
        .collect()
}

/// FR-45 populator helper: inspect a successfully-parsed
/// `MemoryFile` for soft integrity issues and emit a note per
/// issue. Hard parse errors already bail out upstream as a
/// `McpError::invalid_params`; this function runs only on the
/// happy path and lets callers know their memory has recoverable
/// drift (empty name, empty description, frontmatter id ≠
/// filename UUID, etc.).
///
/// Returns an empty Vec when everything checks out.
#[must_use]
pub fn malformed_frontmatter_notes(
    slug: &str,
    filename_id: Uuid,
    file: &MemoryFile,
) -> Vec<Note> {
    let fm = &file.frontmatter;
    let mut notes = Vec::new();
    if fm.name.trim().is_empty() {
        notes.push(
            Note::warn(
                "malformed_frontmatter",
                format!("memory `{slug}` has an empty `name` field"),
            )
            .with_context(json!({ "slug": slug, "field": "name" })),
        );
    }
    if fm.description.trim().is_empty() {
        notes.push(
            Note::warn(
                "malformed_frontmatter",
                format!("memory `{slug}` has an empty `description` field"),
            )
            .with_context(json!({ "slug": slug, "field": "description" })),
        );
    }
    match fm.id {
        None => notes.push(
            Note::warn(
                "malformed_frontmatter",
                format!(
                    "memory `{slug}` has no `id` in frontmatter; expected {filename_id} per FR-028"
                ),
            )
            .with_context(json!({
                "slug": slug,
                "field": "id",
                "expected": filename_id.to_string(),
            })),
        ),
        Some(id) if id != filename_id => notes.push(
            Note::warn(
                "malformed_frontmatter",
                format!(
                    "memory `{slug}` frontmatter id {id} does not match filename UUID {filename_id}"
                ),
            )
            .with_context(json!({
                "slug": slug,
                "field": "id",
                "frontmatter_id": id.to_string(),
                "filename_id": filename_id.to_string(),
            })),
        ),
        _ => {}
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_notes_tail_skips_empty_silently() {
        render_notes_tail(&[]);
    }

    #[test]
    fn issue_to_note_maps_error_severity() {
        let n = issue_to_note(&Issue {
            group: "g".to_string(),
            slug: Some("s".to_string()),
            severity: "error",
            code: "c",
            message: "m".to_string(),
        });
        assert_eq!(n.level, NoteLevel::Error);
        assert_eq!(n.code, "c");
    }

    #[test]
    fn issue_to_note_maps_warning_severity() {
        let n = issue_to_note(&Issue {
            group: "g".to_string(),
            slug: None,
            severity: "warning",
            code: "c",
            message: "m".to_string(),
        });
        assert_eq!(n.level, NoteLevel::Warn);
    }

    #[test]
    fn issue_to_note_defaults_unknown_severity_to_info() {
        let n = issue_to_note(&Issue {
            group: "g".to_string(),
            slug: None,
            severity: "info",
            code: "c",
            message: "m".to_string(),
        });
        assert_eq!(n.level, NoteLevel::Info);
    }
}
