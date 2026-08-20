//! Notes-channel helpers shared by the MCP stdio server and
//! the CLI subcommands. Both surfaces render the same
//! [`mmcp_proto::Note`] vocabulary so a note code an operator sees
//! on the command line matches what an AI caller sees through MCP.
//!
//! The MCP path wraps responses with `ok_json_with_notes` (see
//! `commands::serve`); the CLI path calls [`render_notes_tail`] at
//! the foot of each subcommand's output. Mapping store-side
//! diagnostic findings onto notes is handled by [`findings_to_notes`]
//! so the two renderers stay in lockstep.
//!
//! The module also owns the [`dangling_ref_notes_for`] and
//! [`malformed_frontmatter_notes`] populators that were previously
//! inlined in `commands::serve`. Exposing them here lets every
//! `mmcp feature *` / `mmcp memory read` CLI entry point and the
//! corresponding MCP tool thread the same detection logic without
//! duplicating code.

use mmcp_core::memory::{MemoryFile, MemoryRef};
use mmcp_git::{NativeBackend, Rev};
use mmcp_proto::{Note, NoteLevel};
use mmcp_store::diagnostics::Finding;
use mmcp_store::groups::GroupEntry;
use mmcp_store::memory::list_all_memory_files;
use mmcp_sync::{GroupSyncFailure, PushReport, RemoteManifestFailure};
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

/// Convert a store-side [`Finding`] into a [`Note`] on the notes wire.
///
/// `severity` maps to `level`; `code` flows through unchanged;
/// `group` + optional `slug` ride in `context` so callers can
/// route or group notes without re-parsing the message.
#[must_use]
pub fn finding_to_note(finding: &Finding) -> Note {
    let level = match finding.severity {
        "error" => NoteLevel::Error,
        "warning" => NoteLevel::Warn,
        _ => NoteLevel::Info,
    };
    let mut ctx = serde_json::Map::new();
    ctx.insert("group".to_string(), json!(finding.group));
    if let Some(ref slug) = finding.slug {
        ctx.insert("slug".to_string(), json!(slug));
    }
    Note {
        level,
        code: finding.code.to_string(),
        message: finding.message.clone(),
        context: Some(serde_json::Value::Object(ctx)),
    }
}

/// Map a slice of diagnostic findings onto the notes channel.
#[must_use]
pub fn findings_to_notes(findings: &[Finding]) -> Vec<Note> {
    findings.iter().map(finding_to_note).collect()
}

pub async fn dangling_ref_notes_for(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    depends_on: &[Uuid],
    blocks: &[Uuid],
    superseded_by: Option<&MemoryRef>,
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
    for (field, uuid) in depends_on
        .iter()
        .map(|u| ("depends_on", *u))
        .chain(blocks.iter().map(|u| ("blocks", *u)))
    {
        if !known.contains(&uuid) {
            notes.push(
                Note::warn(
                    "dangling_ref",
                    format!("feature `{slug}` references unresolved target in `{field}`"),
                )
                .with_context(json!({
                    "slug": slug,
                    "field": field,
                    "target": uuid.to_string(),
                    "group": entry.manifest.group_id.to_string(),
                })),
            );
        }
    }
    if let Some(link) = superseded_by
        && !known.contains(&link.target)
    {
        notes.push(
            Note::warn(
                "dangling_ref",
                format!("feature `{slug}` superseded_by target does not resolve locally"),
            )
            .with_context(json!({
                "slug": slug,
                "field": "superseded_by",
                "target": link.target.to_string(),
                "commit": link.commit,
                "group": entry.manifest.group_id.to_string(),
            })),
        );
    }
    notes
}

/// Populator helper: scan a push report for groups whose
/// control-plane push succeeded but whose content plane (git
/// push) did not actually ship bytes. Each such group surfaces
/// as one `sync_partial_failure` warn so callers don't assume
/// silent success.
///
/// Shared by the MCP `sync_push` tool and the CLI `mmcp push` /
/// `mmcp sync` commands so both emit the same code with the same
/// context shape (`group`, `stage`, `remote`). Reads the remote name
/// straight off `report` (one push attempt is always attributed to
/// its own `BoundRemote` since the multi-remote restructuring), so
/// unlike the single-remote era this needs no separate label
/// argument from the caller.
#[must_use]
pub fn sync_push_partial_failure_notes(report: &PushReport) -> Vec<Note> {
    report
        .iter_partial_failures()
        .map(|(remote_name, p)| {
            Note::warn(
                "sync_partial_failure",
                format!(
                    "push for group {} via remote '{remote_name}' did not ship content (transport error or unsupported backend)",
                    p.group_id
                ),
            )
            .with_context(json!({
                "group": p.group_id.to_string(),
                "stage": "push",
                "remote": remote_name,
            }))
        })
        .collect()
}

/// Turn a sync report's `failed` list into one `sync_group_failed` note per group.
#[must_use]
pub fn sync_group_failure_notes(
    failed: &[GroupSyncFailure],
    stage: &str,
    server_url: &str,
) -> Vec<Note> {
    failed
        .iter()
        .map(|f| {
            Note::error(
                "sync_group_failed",
                format!("{stage} failed for group {}: {}", f.group_id, f.error),
            )
            .with_context(json!({
                "group": f.group_id.to_string(),
                "stage": stage,
                "server_url": server_url,
            }))
        })
        .collect()
}

/// Turn a `fetch` / `pull` report's `manifest_failures` list into one
/// `sync_manifest_failed` note per unreachable remote.
///
/// Distinct from [`sync_group_failure_notes`]: a manifest failure has
/// no group id (the poll never got far enough to discover one), so it
/// gets its own note code and context shape (`remote` instead of
/// `group`) rather than forcing a placeholder group id through the
/// existing helper.
#[must_use]
pub fn sync_manifest_failure_notes(failed: &[RemoteManifestFailure], stage: &str) -> Vec<Note> {
    failed
        .iter()
        .map(|f| {
            Note::error(
                "sync_manifest_failed",
                format!(
                    "{stage} could not read remote '{}' manifest: {}",
                    f.remote_name, f.error
                ),
            )
            .with_context(json!({
                "remote": f.remote_name,
                "stage": stage,
            }))
        })
        .collect()
}

/// Map a [`mmcp_store::IdValidation`] outcome onto notes.
/// Returns an empty vector for the silent `Match` case so no note
/// is emitted; the two mismatch variants surface as
/// `id_mismatch_accepted` (frontmatter wins) and
/// `id_mismatch_forced` (caller bypassed the filename rejection).
///
/// Shared by the MCP write tool bodies in `commands::serve` and
/// the `mmcp memory write / edit / edit-body` CLI subcommands so
/// both surfaces emit the same code with the same context shape.
#[must_use]
pub fn id_validation_to_notes(validation: &mmcp_store::IdValidation, slug: &str) -> Vec<Note> {
    match validation {
        mmcp_store::IdValidation::Match => Vec::new(),
        mmcp_store::IdValidation::MismatchAccepted {
            filename,
            frontmatter,
        } => vec![
            Note::warn(
                "id_mismatch_accepted",
                format!(
                    "memory `{slug}` filename id {filename} disagrees with frontmatter id {frontmatter}; frontmatter is source of truth, write accepted"
                ),
            )
            .with_context(json!({
                "slug": slug,
                "filename": filename.to_string(),
                "frontmatter": frontmatter.to_string(),
            })),
        ],
        mmcp_store::IdValidation::MismatchForced {
            filename,
            frontmatter,
        } => vec![
            Note::warn(
                "id_mismatch_forced",
                format!(
                    "memory `{slug}` filename id {filename} disagrees with frontmatter id {frontmatter}; write forced past the rejection rule"
                ),
            )
            .with_context(json!({
                "slug": slug,
                "filename": filename.to_string(),
                "frontmatter": frontmatter.to_string(),
            })),
        ],
    }
}

/// Populator helper: inspect a successfully-parsed
/// `MemoryFile` for soft integrity issues and emit a note per
/// issue. Hard parse errors already bail out upstream as a
/// `McpError::invalid_params`; this function runs only on the
/// happy path and lets callers know their memory has recoverable
/// drift (empty name, empty description, frontmatter id ≠
/// filename UUID, etc.).
///
/// Returns an empty Vec when everything checks out.
#[must_use]
pub fn malformed_frontmatter_notes(slug: &str, filename_id: Uuid, file: &MemoryFile) -> Vec<Note> {
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn render_notes_tail_skips_empty_silently() {
        render_notes_tail(&[]);
    }

    #[test]
    fn finding_to_note_maps_error_severity() {
        let n = finding_to_note(&Finding {
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
    fn finding_to_note_maps_warning_severity() {
        let n = finding_to_note(&Finding {
            group: "g".to_string(),
            slug: None,
            severity: "warning",
            code: "c",
            message: "m".to_string(),
        });
        assert_eq!(n.level, NoteLevel::Warn);
    }

    #[test]
    fn finding_to_note_defaults_unknown_severity_to_info() {
        let n = finding_to_note(&Finding {
            group: "g".to_string(),
            slug: None,
            severity: "info",
            code: "c",
            message: "m".to_string(),
        });
        assert_eq!(n.level, NoteLevel::Info);
    }
}
