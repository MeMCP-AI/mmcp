//! Notes about the project's and the user's CLAUDE.md.

use std::path::{Path, PathBuf};

use mmcp_core::config::{ConfigKey, ConfigScope, NoticeValue};
use serde_json::json;

use super::{InitClaudeAction, InitClaudeConflict, action_wire};

/// Code of the note for a project CLAUDE.md that does not exist.
const CODE_MISSING: &str = "claude_md_missing";

/// Code of the note for a CLAUDE.md without an mmcp-managed block.
const CODE_UNMANAGED: &str = "claude_md_unmanaged";

/// Code of the note for a CLAUDE.md whose managed block is not the current render.
const CODE_UPDATE_AVAILABLE: &str = "claude_md_update_available";

/// Code of the note for a CLAUDE.md carrying one fence marker without its counterpart.
/// A defect report, never gated by a notice key.
const CODE_PARTIAL_FENCE: &str = "claude_md_partial_fence";

/// Compute notes about the project's and the user's CLAUDE.md.
///
/// Read-only: the files are inspected, never written.
/// `bootstrap_context` emits these through the standard notes channel.
/// The project file, when a project root exists, reports a missing file, an unmanaged file, a partial fence or an available update.
/// The user-level file reports a partial fence or an available update only, since mmcp does not require it.
/// Each file yields at most one note, naming its absolute path.
/// `project_notice` (`notice.md.project`) and `user_notice` (`notice.md.user`) each turn off the suggestions of their own file; a partial fence is reported either way.
/// While `project_notice` is on, a project-file suggestion ends with the sentence offering to turn it off.
pub fn claude_md_notes(
    project_root: Option<&Path>,
    user_claude_md: Option<&Path>,
    project_notice: NoticeValue,
    user_notice: NoticeValue,
) -> Vec<mmcp_proto::Note> {
    let project_note = project_root
        .and_then(|root| {
            project_claude_md_note(&absolute_claude_md_path(
                &root.join(crate::commands::claude::CLAUDE_MD_FILE_NAME),
            ))
        })
        .and_then(|note| gate_notice(note, project_notice))
        .map(offer_to_turn_off_when_suggesting);
    let user_note = user_claude_md
        .and_then(|path| user_claude_md_note(&absolute_claude_md_path(path)))
        .and_then(|note| gate_notice(note, user_notice));
    project_note.into_iter().chain(user_note).collect()
}

/// The note, or `None` when `value` turns off the notices of its file and the note is a suggestion.
/// A partial-fence note always passes.
fn gate_notice(note: mmcp_proto::Note, value: NoticeValue) -> Option<mmcp_proto::Note> {
    match value {
        NoticeValue::On => Some(note),
        NoticeValue::Off if note.code == CODE_PARTIAL_FENCE => Some(note),
        NoticeValue::Off => None,
    }
}

/// The project-file note with the offer to turn it off appended to its message, unless it is a partial-fence report.
fn offer_to_turn_off_when_suggesting(mut note: mmcp_proto::Note) -> mmcp_proto::Note {
    if note.code != CODE_PARTIAL_FENCE {
        note.message = format!(
            "{} You may also offer to turn this notice off with config (action=set, scope={}, key={}, value={}).",
            note.message,
            ConfigScope::Local.as_str(),
            ConfigKey::NoticeMdProject.as_str(),
            NoticeValue::Off.as_str(),
        );
    }
    note
}

/// Absolute form of a CLAUDE.md path, since `init_claude` otherwise resolves a relative one against its cwd.
fn absolute_claude_md_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), error = %err, "CLAUDE.md path not made absolute");
        path.to_path_buf()
    })
}

/// Note for the project's CLAUDE.md, `None` when its block is current.
fn project_claude_md_note(path: &Path) -> Option<mmcp_proto::Note> {
    let display = path.display();
    if !path.exists() {
        return Some(
            mmcp_proto::Note::warn(
                CODE_MISSING,
                format!(
                    "CLAUDE.md is missing at {display}. Present the proposed block to the user. Run init_claude (action=override) only after their approval."
                ),
            )
            .with_context(claude_md_note_context(
                path,
                InitClaudeAction::Override,
                None,
            )),
        );
    }
    let body = read_claude_md(path)?;
    match crate::commands::claude::scan_fence(&body) {
        Ok(None) => Some(
            mmcp_proto::Note::warn(
                CODE_UNMANAGED,
                format!(
                    "CLAUDE.md at {display} has no mmcp-managed block. Present the proposed block to the user. Run init_claude (action=append) only after their approval; action=convert instead splits the existing rule content into typed memories."
                ),
            )
            .with_context(claude_md_note_context(path, InitClaudeAction::Append, None)),
        ),
        scan => fenced_claude_md_note(path, scan, None),
    }
}

/// Note for the user-level `~/.claude/CLAUDE.md`, `None` when it is missing, unmanaged or current.
/// The file sits outside any repository, so `init_claude` reads it as untracked and needs a conflict answer.
fn user_claude_md_note(path: &Path) -> Option<mmcp_proto::Note> {
    if !path.exists() {
        return None;
    }
    let body = read_claude_md(path)?;
    fenced_claude_md_note(
        path,
        crate::commands::claude::scan_fence(&body),
        Some(InitClaudeConflict::BackupOverride),
    )
}

/// Note for a CLAUDE.md carrying fence markers: an available update or a partial fence.
/// `None` when the file carries no marker or its block is current.
fn fenced_claude_md_note(
    path: &Path,
    scan: Result<
        Option<crate::commands::claude::Fence<'_>>,
        crate::commands::claude::PartialFenceError,
    >,
    on_conflict: Option<InitClaudeConflict>,
) -> Option<mmcp_proto::Note> {
    let display = path.display();
    match scan {
        Ok(None) => None,
        Ok(Some(fence)) if fence.is_current() => None,
        Ok(Some(fence)) => {
            let current = fence.version;
            let available = crate::commands::claude::BLOCK_VERSION;
            let mut context = claude_md_note_context(path, InitClaudeAction::Append, on_conflict);
            context["current_version"] = json!(current);
            context["available_version"] = json!(available);
            Some(
                mmcp_proto::Note::warn(
                    CODE_UPDATE_AVAILABLE,
                    format!(
                        "CLAUDE.md at {display} carries mmcp block {current}. Block {available} is available. Present the proposed block to the user. Run init_claude (action=append) only after their approval."
                    ),
                )
                .with_context(context),
            )
        }
        Err(partial) => Some(
            mmcp_proto::Note::warn(CODE_PARTIAL_FENCE, format!("{display}: {partial}."))
                .with_context(json!({ "path": path.to_string_lossy() })),
        ),
    }
}

/// Context of a CLAUDE.md note proposing the managed block, `suggested_args` included.
fn claude_md_note_context(
    path: &Path,
    action: InitClaudeAction,
    on_conflict: Option<InitClaudeConflict>,
) -> serde_json::Value {
    let path_wire = path.to_string_lossy();
    let mut suggested_args = json!({
        "action": action_wire(action),
        "path": path_wire,
    });
    if let Some(answer) = on_conflict {
        suggested_args["on_conflict"] = json!(answer);
    }
    json!({
        "path": path_wire,
        "proposed_block": crate::commands::claude::render_block(),
        "suggested_tool": "init_claude",
        "suggested_args": suggested_args,
    })
}

/// Read a CLAUDE.md, logging and skipping a file that cannot be read.
fn read_claude_md(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(body) => Some(body),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "CLAUDE.md unreadable; no note emitted for it");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use tempfile::TempDir;

    use super::*;

    /// The approved sentence closing every project-file suggestion, verbatim.
    const OFFER_SENTENCE: &str = "You may also offer to turn this notice off with config (action=set, scope=local, key=notice.md.project, value=off).";

    /// Version tag of an older mmcp block, for staleness fixtures.
    const OLDER_BLOCK_VERSION: &str = "v1";

    /// A fenced block carrying the older version tag and an older body.
    fn older_fenced_block() -> String {
        use crate::commands::claude::{BLOCK_VERSION, begin_marker, end_marker};
        format!(
            "{}\n## MANDATORY: Re-read rules at every checkpoint\n{}",
            begin_marker().replacen(BLOCK_VERSION, OLDER_BLOCK_VERSION, 1),
            end_marker().replacen(BLOCK_VERSION, OLDER_BLOCK_VERSION, 1),
        )
    }

    /// Absolute wire form of a note's path.
    fn absolute_wire(path: &Path) -> String {
        std::path::absolute(path)
            .expect("absolute path")
            .to_string_lossy()
            .into_owned()
    }

    fn note_context(note: &mmcp_proto::Note) -> &serde_json::Value {
        note.context.as_ref().expect("note carries a context")
    }

    /// Notes with both keys at their default.
    fn notes_on(
        project_root: Option<&Path>,
        user_claude_md: Option<&Path>,
    ) -> Vec<mmcp_proto::Note> {
        claude_md_notes(
            project_root,
            user_claude_md,
            NoticeValue::On,
            NoticeValue::On,
        )
    }

    #[test]
    fn claude_md_notes_flags_missing_file() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("CLAUDE.md");
        let notes = notes_on(Some(tmp.path()), None);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "claude_md_missing");
        assert_eq!(notes[0].level, mmcp_proto::NoteLevel::Warn);
        assert!(notes[0].message.contains(&path.display().to_string()));
        let context = note_context(&notes[0]);
        assert_eq!(context["path"], json!(absolute_wire(&path)));
        assert_eq!(
            context["proposed_block"],
            json!(crate::commands::claude::render_block())
        );
        assert_eq!(context["suggested_tool"], json!("init_claude"));
        assert_eq!(
            context["suggested_args"],
            json!({ "action": "override", "path": absolute_wire(&path) })
        );
    }

    #[test]
    fn claude_md_notes_flags_unmanaged_file() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("CLAUDE.md");
        std::fs::write(&path, "# Legacy\n\nHand-authored without any mmcp fence.\n")
            .expect("write claude");
        let notes = notes_on(Some(tmp.path()), None);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "claude_md_unmanaged");
        assert_eq!(notes[0].level, mmcp_proto::NoteLevel::Warn);
        assert!(notes[0].message.contains(&path.display().to_string()));
        let context = note_context(&notes[0]);
        assert_eq!(context["path"], json!(absolute_wire(&path)));
        assert_eq!(
            context["suggested_args"],
            json!({ "action": "append", "path": absolute_wire(&path) })
        );
    }

    #[test]
    fn claude_md_notes_is_silent_when_the_block_is_the_current_render() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            format!(
                "# Managed\n\nOwn notes.\n\n{}\n",
                crate::commands::claude::render_block()
            ),
        )
        .expect("write claude");
        let notes = notes_on(Some(tmp.path()), None);
        assert!(
            notes.is_empty(),
            "a current block produces no note: {notes:?}"
        );
    }

    #[test]
    fn claude_md_notes_offers_the_update_for_an_older_block() {
        use crate::commands::claude::{BLOCK_VERSION, render_block};
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("CLAUDE.md");
        std::fs::write(&path, format!("# Old\n\n{}\n", older_fenced_block()))
            .expect("write claude");

        let notes = notes_on(Some(tmp.path()), None);

        assert_eq!(notes.len(), 1, "one note per file: {notes:?}");
        let note = &notes[0];
        assert_eq!(note.code, "claude_md_update_available");
        assert_eq!(note.level, mmcp_proto::NoteLevel::Warn);
        assert_eq!(
            note.message,
            format!(
                "CLAUDE.md at {} carries mmcp block {OLDER_BLOCK_VERSION}. Block {BLOCK_VERSION} is available. Present the proposed block to the user. Run init_claude (action=append) only after their approval. {OFFER_SENTENCE}",
                path.display()
            )
        );
        let context = note_context(note);
        assert_eq!(context["path"], json!(absolute_wire(&path)));
        assert_eq!(context["current_version"], json!(OLDER_BLOCK_VERSION));
        assert_eq!(context["available_version"], json!(BLOCK_VERSION));
        assert_eq!(context["proposed_block"], json!(render_block()));
        assert_eq!(context["suggested_tool"], json!("init_claude"));
        assert_eq!(
            context["suggested_args"],
            json!({ "action": "append", "path": absolute_wire(&path) })
        );
    }

    #[test]
    fn claude_md_notes_flags_a_current_version_fence_with_a_non_canonical_body() {
        use crate::commands::claude::{BLOCK_VERSION, begin_marker, end_marker};
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            format!(
                "# Managed\n\n{}\nedited by hand\n{}\n",
                begin_marker(),
                end_marker()
            ),
        )
        .expect("write claude");
        let notes = notes_on(Some(tmp.path()), None);
        assert_eq!(notes.len(), 1, "one note per file: {notes:?}");
        assert_eq!(notes[0].code, "claude_md_update_available");
        let context = note_context(&notes[0]);
        assert_eq!(context["current_version"], json!(BLOCK_VERSION));
        assert_eq!(context["available_version"], json!(BLOCK_VERSION));
    }

    #[test]
    fn claude_md_notes_flags_a_partial_fence() {
        use crate::commands::claude::begin_marker;
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("CLAUDE.md");
        std::fs::write(&path, format!("{}\nno end marker\n", begin_marker()))
            .expect("write claude");
        let notes = notes_on(Some(tmp.path()), None);
        assert_eq!(notes.len(), 1, "one note per file: {notes:?}");
        assert_eq!(notes[0].code, "claude_md_partial_fence");
        assert_eq!(notes[0].level, mmcp_proto::NoteLevel::Warn);
        assert!(notes[0].message.contains(&path.display().to_string()));
        assert!(notes[0].message.contains("partial mmcp fence"));
    }

    #[test]
    fn claude_md_notes_offers_the_update_for_a_stale_user_file_without_project_root() {
        use crate::commands::claude::{BLOCK_VERSION, render_block};
        let tmp = TempDir::new().expect("tempdir");
        let user_claude_md = tmp
            .path()
            .join("user-home")
            .join(".claude")
            .join("CLAUDE.md");
        std::fs::create_dir_all(user_claude_md.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &user_claude_md,
            format!("# Global\n\n{}\n", older_fenced_block()),
        )
        .expect("write user claude");

        let notes = notes_on(None, Some(&user_claude_md));

        assert_eq!(notes.len(), 1, "one note for the user file: {notes:?}");
        let note = &notes[0];
        assert_eq!(note.code, "claude_md_update_available");
        assert_eq!(note.level, mmcp_proto::NoteLevel::Warn);
        assert!(note.message.contains(&user_claude_md.display().to_string()));
        let context = note_context(note);
        assert_eq!(context["path"], json!(absolute_wire(&user_claude_md)));
        assert_eq!(context["current_version"], json!(OLDER_BLOCK_VERSION));
        assert_eq!(context["available_version"], json!(BLOCK_VERSION));
        assert_eq!(context["proposed_block"], json!(render_block()));
        assert_eq!(context["suggested_tool"], json!("init_claude"));
        assert_eq!(
            context["suggested_args"],
            json!({
                "action": "append",
                "path": absolute_wire(&user_claude_md),
                "on_conflict": "backup_override",
            })
        );
    }

    #[test]
    fn claude_md_notes_is_silent_for_a_missing_unmanaged_or_current_user_file() {
        let tmp = TempDir::new().expect("tempdir");
        let user_claude_md = tmp.path().join("CLAUDE.md");
        assert!(notes_on(None, Some(&user_claude_md)).is_empty());

        std::fs::write(&user_claude_md, "# Personal\n\nNo fence.\n").expect("write");
        assert!(notes_on(None, Some(&user_claude_md)).is_empty());

        std::fs::write(
            &user_claude_md,
            format!(
                "# Personal\n\n{}\n",
                crate::commands::claude::render_block()
            ),
        )
        .expect("write");
        assert!(notes_on(None, Some(&user_claude_md)).is_empty());
    }

    #[test]
    fn claude_md_notes_checks_the_project_and_user_files_independently() {
        let tmp = TempDir::new().expect("tempdir");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).expect("mkdir project");
        let project_claude_md = project.join("CLAUDE.md");
        std::fs::write(&project_claude_md, older_fenced_block()).expect("write project");
        let user_claude_md = tmp.path().join("user-CLAUDE.md");
        std::fs::write(&user_claude_md, older_fenced_block()).expect("write user");

        let notes = notes_on(Some(&project), Some(&user_claude_md));

        let paths: Vec<&serde_json::Value> = notes
            .iter()
            .map(|note| &note_context(note)["path"])
            .collect();
        assert_eq!(
            paths,
            vec![
                &json!(absolute_wire(&project_claude_md)),
                &json!(absolute_wire(&user_claude_md)),
            ]
        );
        assert!(
            notes
                .iter()
                .all(|note| note.code == "claude_md_update_available")
        );
    }

    #[test]
    fn claude_md_notes_is_silent_without_project_root_or_user_file() {
        assert!(notes_on(None, None).is_empty());
    }

    /// A project directory with an optional CLAUDE.md and a user CLAUDE.md carrying a stale block.
    struct BothFilesFixture {
        _tmp: TempDir,
        project: PathBuf,
        user_claude_md: PathBuf,
    }

    impl BothFilesFixture {
        fn with_project_claude_md(project_claude_md: Option<&str>) -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let project = tmp.path().join("project");
            std::fs::create_dir_all(&project).expect("mkdir project");
            if let Some(body) = project_claude_md {
                std::fs::write(project.join("CLAUDE.md"), body).expect("write project");
            }
            let user_claude_md = tmp.path().join("user-CLAUDE.md");
            std::fs::write(&user_claude_md, older_fenced_block()).expect("write user");
            Self {
                _tmp: tmp,
                project,
                user_claude_md,
            }
        }

        fn codes(&self, project_notice: NoticeValue, user_notice: NoticeValue) -> Vec<String> {
            claude_md_notes(
                Some(&self.project),
                Some(&self.user_claude_md),
                project_notice,
                user_notice,
            )
            .into_iter()
            .map(|note| note.code)
            .collect()
        }
    }

    #[test]
    fn project_off_suppresses_missing_and_emits_nothing_else_for_the_project_file() {
        let fixture = BothFilesFixture::with_project_claude_md(None);
        assert_eq!(
            fixture.codes(NoticeValue::Off, NoticeValue::On),
            vec!["claude_md_update_available"],
            "only the user file's note remains"
        );
    }

    #[test]
    fn project_off_suppresses_unmanaged_and_update_available_of_the_project_file() {
        let unmanaged = BothFilesFixture::with_project_claude_md(Some("# Legacy\n"));
        assert_eq!(
            unmanaged.codes(NoticeValue::Off, NoticeValue::On),
            vec!["claude_md_update_available"]
        );
        let stale = BothFilesFixture::with_project_claude_md(Some(&older_fenced_block()));
        assert_eq!(
            stale.codes(NoticeValue::Off, NoticeValue::On),
            vec!["claude_md_update_available"]
        );
    }

    #[test]
    fn user_off_suppresses_the_user_file_update_notice_and_emits_nothing_else_for_it() {
        let fixture = BothFilesFixture::with_project_claude_md(None);
        assert_eq!(
            fixture.codes(NoticeValue::On, NoticeValue::Off),
            vec!["claude_md_missing"],
            "only the project file's note remains"
        );
    }

    #[test]
    fn each_key_leaves_the_other_files_notices_untouched() {
        let fixture = BothFilesFixture::with_project_claude_md(Some(&older_fenced_block()));
        assert_eq!(
            fixture.codes(NoticeValue::Off, NoticeValue::On),
            vec!["claude_md_update_available"]
        );
        assert_eq!(
            fixture.codes(NoticeValue::On, NoticeValue::Off),
            vec!["claude_md_update_available"]
        );
        assert_eq!(
            fixture.codes(NoticeValue::Off, NoticeValue::Off),
            Vec::<String>::new()
        );
        assert_eq!(
            fixture.codes(NoticeValue::On, NoticeValue::On),
            vec!["claude_md_update_available", "claude_md_update_available"]
        );
    }

    #[test]
    fn partial_fence_is_emitted_for_either_file_with_both_keys_off() {
        use crate::commands::claude::begin_marker;
        let partial = format!("{}\nno end marker\n", begin_marker());
        let fixture = BothFilesFixture::with_project_claude_md(Some(&partial));
        std::fs::write(&fixture.user_claude_md, &partial).expect("write user");

        assert_eq!(
            fixture.codes(NoticeValue::Off, NoticeValue::Off),
            vec!["claude_md_partial_fence", "claude_md_partial_fence"]
        );
    }

    #[test]
    fn project_on_suggestions_end_with_the_approved_sentence() {
        let missing = BothFilesFixture::with_project_claude_md(None);
        let unmanaged = BothFilesFixture::with_project_claude_md(Some("# Legacy\n"));
        let stale = BothFilesFixture::with_project_claude_md(Some(&older_fenced_block()));
        for fixture in [missing, unmanaged, stale] {
            let notes = notes_on(Some(&fixture.project), None);
            assert_eq!(notes.len(), 1);
            assert!(
                notes[0].message.ends_with(&format!(" {OFFER_SENTENCE}")),
                "{}",
                notes[0].message
            );
        }
    }

    #[test]
    fn the_sentence_leaves_the_existing_message_text_untouched() {
        let fixture = BothFilesFixture::with_project_claude_md(None);
        let path = fixture.project.join("CLAUDE.md");

        let notes = notes_on(Some(&fixture.project), None);

        assert_eq!(
            notes[0].message,
            format!(
                "CLAUDE.md is missing at {}. Present the proposed block to the user. Run init_claude (action=override) only after their approval. {OFFER_SENTENCE}",
                path.display()
            )
        );
    }

    #[test]
    fn user_file_notice_never_carries_the_config_sentence() {
        let fixture = BothFilesFixture::with_project_claude_md(None);
        let notes = notes_on(None, Some(&fixture.user_claude_md));
        assert_eq!(notes.len(), 1);
        assert!(
            !notes[0].message.contains("config (action="),
            "{}",
            notes[0].message
        );
    }

    #[test]
    fn a_partial_fence_report_never_carries_the_config_sentence() {
        use crate::commands::claude::begin_marker;
        let partial = format!("{}\nno end marker\n", begin_marker());
        let fixture = BothFilesFixture::with_project_claude_md(Some(&partial));
        let notes = notes_on(Some(&fixture.project), None);
        assert_eq!(notes[0].code, "claude_md_partial_fence");
        assert!(!notes[0].message.contains("config (action="));
    }
}
