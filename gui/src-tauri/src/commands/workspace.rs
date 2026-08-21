//! Workspace commands — runtime control over the "reference point"
//! directory that anchors `.mmcp.toml` discovery.
//!
//! The frontend persists `reference_point` via the normal settings
//! round-trip (`save_settings`). This command then asks the backend
//! to reload `ProjectConfig::sync` against the new path, rebuild
//! the `SyncBundle`, and respawn the reachability probe so the
//! status bar reflects the new server without requiring an app
//! restart.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::sync::SyncStatusDto;
use crate::error::{GuiDialogError, GuiResult};
use crate::state::AppState;
use crate::{emit_reachability_not_applicable, probe_loop};

fn normalise(path: Option<String>) -> Option<PathBuf> {
    let raw = path?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    candidate.is_dir().then_some(candidate)
}

/// What a sync-bundle rebuild does with the reachability probe,
/// given the fresh bundle's `probe_url`.
///
/// A pure decision, factored out so it is unit-testable without a
/// real `AppHandle`, and shared by EVERY place that (re)spawns the
/// probe: `set_reference_point` below (a workspace switch) and
/// `lib::run`'s setup closure (app startup).
///
/// The `None` arm covers a `direct-git` default (or no sync at all),
/// where a stale online/offline reading from a PREVIOUS workspace,
/// or no reading at all yet, must not linger on screen because
/// nothing told the frontend the probe does not apply here.
///
/// `pub(crate)`: consumed from `crate::run`'s setup closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeAction {
    /// Respawn the probe against this URL.
    Spawn(String),
    /// No probe applies to the new default remote: reset the badge
    /// instead of leaving a stale reading on screen.
    Reset,
}

pub(crate) fn probe_action_for(probe_url: Option<String>) -> ProbeAction {
    match probe_url {
        Some(url) => ProbeAction::Spawn(url),
        None => ProbeAction::Reset,
    }
}

/// Open a native folder picker parented to the main window,
/// regardless of which webview dispatched the command. Keeps the
/// dialog visually anchored to the primary app surface so the
/// Settings child window doesn't end up with a dialog that looks
/// disconnected from the work the user is doing.
#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    default_path: Option<String>,
    title: Option<String>,
) -> GuiResult<Option<String>> {
    let main = app
        .get_webview_window("main")
        .ok_or(GuiDialogError::NoMainWindow)?;

    let mut builder = app.dialog().file();
    if let Some(t) = title {
        builder = builder.set_title(t);
    }
    if let Some(p) = default_path {
        let candidate = PathBuf::from(&p);
        if candidate.exists() {
            builder = builder.set_directory(candidate);
        }
    }
    builder = builder.set_parent(&main);

    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx.await.map_err(|_| GuiDialogError::ChannelClosed)?;

    Ok(picked.and_then(|p| {
        p.into_path()
            .ok()
            .map(|pb| pb.to_string_lossy().into_owned())
    }))
}

#[tauri::command]
pub async fn set_reference_point(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> GuiResult<SyncStatusDto> {
    let resolved = normalise(path);
    let snapshot = state.rebuild_sync(resolved.as_deref()).await?;

    // Swap probes. An aborted task just drops its future; no need to
    // await it, the old `get_manifest()` request may still finish on
    // the wire but its result is discarded.
    let mut probe = state.probe.lock().await;
    if let Some(old) = probe.take() {
        old.abort();
    }
    match probe_action_for(snapshot.as_ref().and_then(|s| s.probe_url.clone())) {
        ProbeAction::Spawn(url) => {
            let fresh = tauri::async_runtime::spawn(probe_loop(app.clone(), url));
            *probe = Some(fresh);
        }
        ProbeAction::Reset => emit_reachability_not_applicable(&app),
    }

    Ok(SyncStatusDto {
        configured: snapshot.is_some(),
        remotes_summary: snapshot.map(|s| s.remotes_summary),
        // `rebuild_sync` above already cleared `state.sync_error` on
        // this successful path (it only reaches here via `?`, which
        // would have returned early on failure instead).
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolved `probe_url` respawns the probe against it. Covers
    /// the ordinary `mmcp-server`-default case, unchanged by Fix 3.
    #[test]
    fn probe_action_for_some_url_spawns_a_fresh_probe() {
        assert_eq!(
            probe_action_for(Some("https://a.example.com".to_string())),
            ProbeAction::Spawn("https://a.example.com".to_string())
        );
    }

    /// Falsification target for the regression this fix closes: no
    /// `probe_url` (no sync configured, or a `direct-git` default)
    /// must produce `Reset`, not silently leave the previous probe's
    /// last reading in place. Before this fix, `set_reference_point`
    /// had no corresponding branch at all, the `None` case was a
    /// silent no-op.
    #[test]
    fn probe_action_for_no_url_resets_the_badge() {
        assert_eq!(probe_action_for(None), ProbeAction::Reset);
    }
}
