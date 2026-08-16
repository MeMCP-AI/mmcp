//! Error type returned to clients when a tool call cannot be served.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Failures returned to MCP clients.
///
/// Each variant is serializable so the transport layer can emit them
/// in the error channel without re-wrapping.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ProtoError {
    /// The caller is not authenticated.
    #[error("not authenticated: {0}")]
    Unauthenticated(String),

    /// The caller is authenticated but not permitted on this memory
    /// or group.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The requested memory, group, or version does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// One or more mandatory memories have not been read this
    /// session. Each entry is the unread memory's slug, so callers
    /// can act on the list programmatically instead of re-parsing a
    /// joined string.
    #[error("mandatory memories unread: {}", .slugs.join(", "))]
    MandatoryUnread { slugs: Vec<String> },

    /// Request payload did not match the schema.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Server-side failure. Contains a short description suitable
    /// for display; the full error is logged server-side.
    #[error("internal error: {0}")]
    Internal(String),

    /// The requested tool exists in the protocol but is not
    /// implemented on this peer yet. Callers should treat it as a
    /// capability gap to surface to the user, not as a transient
    /// failure.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn not_implemented_serializes_with_tagged_kind() {
        let err = ProtoError::NotImplemented("verify_memory".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"not_implemented","message":"verify_memory"}"#
        );
    }

    #[test]
    fn not_implemented_round_trips_through_json() {
        let err = ProtoError::NotImplemented("diff_memory is wired by the client".to_string());
        let json = serde_json::to_string(&err).unwrap();
        let parsed: ProtoError = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, err);
    }

    #[test]
    fn mandatory_unread_carries_the_slug_list_as_a_structured_field() {
        let err = ProtoError::MandatoryUnread {
            slugs: vec![
                "global-coding-rules".to_string(),
                "global-git-conventions".to_string(),
            ],
        };
        match &err {
            ProtoError::MandatoryUnread { slugs } => {
                assert_eq!(slugs, &["global-coding-rules", "global-git-conventions"]);
            }
            other => panic!("expected MandatoryUnread, got {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            "mandatory memories unread: global-coding-rules, global-git-conventions"
        );
    }

    #[test]
    fn mandatory_unread_round_trips_through_json_as_a_slug_array() {
        let err = ProtoError::MandatoryUnread {
            slugs: vec!["global-coding-rules".to_string()],
        };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"mandatory_unread","message":{"slugs":["global-coding-rules"]}}"#
        );
        let parsed: ProtoError = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, err);
    }
}
