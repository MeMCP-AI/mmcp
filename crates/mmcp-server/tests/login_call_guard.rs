#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Mechanical guard: every login of the auth routes goes through the one function that cycles the session id.
//!
//! The passkey login cannot complete without an authenticator in the test graph, so its call is guarded here.
//! The password and OAuth logins are also proven end to end in `credential_change_sessions.rs`.

use std::fs;
use std::path::Path;

/// Handlers that log a user in after the user proved a credential.
const LOGIN_HANDLERS: &[&str] = &["login", "oauth_callback", "passkey_login_finish"];

/// Functions allowed to call `AuthSession::login` themselves.
const DIRECT_LOGIN_CALLERS: &[&str] = &[
    "login_on_a_fresh_session_id",
    "restamp_session_after_credential_change",
];

const FRESH_ID_LOGIN: &str = "login_on_a_fresh_session_id(";

fn auth_routes_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes/auth.rs"))
        .expect("read the auth routes source")
}

/// Splits the source into `(function name, text)` pairs at every top-level `fn`.
fn top_level_functions(source: &str) -> Vec<(String, String)> {
    let mut functions: Vec<(String, String)> = Vec::new();
    for line in source.lines() {
        let declaration = ["pub async fn ", "pub(crate) async fn ", "async fn ", "fn "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix));
        if let Some(rest) = declaration {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            functions.push((name, String::new()));
        }
        if let Some((_, text)) = functions.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    functions
}

#[test]
fn only_the_fresh_id_login_and_the_restamp_call_auth_session_login() {
    for (name, text) in top_level_functions(&auth_routes_source()) {
        if text.contains(".login(") {
            assert!(
                DIRECT_LOGIN_CALLERS.contains(&name.as_str()),
                "{name} calls AuthSession::login directly and keeps a planted session id"
            );
        }
    }
}

#[test]
fn every_login_handler_logs_in_on_a_fresh_session_id() {
    let functions = top_level_functions(&auth_routes_source());
    for handler in LOGIN_HANDLERS {
        let (_, text) = functions
            .iter()
            .find(|(name, _)| name == handler)
            .unwrap_or_else(|| panic!("{handler} no longer exists in the auth routes"));
        assert!(
            text.contains(FRESH_ID_LOGIN),
            "{handler} must log in through login_on_a_fresh_session_id"
        );
    }
}
