//! Authentication and authorization for mmcp.
//!
//! Wires together password hashing (`argon2`), PASETO token issuance and
//! validation (`rusty_paseto`), session middleware (`axum-login` +
//! `tower-sessions`), and OAuth / passkey flows (`oauth2-passkey-axum`).
//!
//! Exposes a small surface that the server binary plugs into its `axum`
//! router. The client binary consumes only the token and keychain helpers.
