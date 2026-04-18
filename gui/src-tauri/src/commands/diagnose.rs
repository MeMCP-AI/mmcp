//! Diagnose command — returns the full DiagReport verbatim.
//!
//! `DiagReport` is already serde-friendly so no DTO wrapping is
//! needed; the frontend consumes the same shape mmcp-store emits.

use mmcp_store::{DiagReport, diagnose_all};
use tauri::State;

use crate::error::GuiResult;
use crate::state::AppState;

#[tauri::command]
pub async fn run_diagnose(state: State<'_, AppState>) -> GuiResult<DiagReport> {
    Ok(diagnose_all(&state.backend, &state.index).await)
}
