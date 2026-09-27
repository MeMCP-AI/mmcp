//! Refusal of a successful tool result lacking the structured content
//! its declared output schema requires.

/// A successful result served for a tool declaring an output schema
/// carried no structured content.
///
/// Every tool payload is built by the JSON result helpers, which set
/// the structured content; its absence means a helper was bypassed, so
/// the result is refused rather than patched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "tool `{tool}` declares an output schema but its successful result carries no structured content"
)]
pub(crate) struct MissingStructuredContent {
    /// Name of the tool whose result was refused.
    pub(crate) tool: String,
}

impl From<MissingStructuredContent> for rmcp::ErrorData {
    fn from(error: MissingStructuredContent) -> Self {
        let data = serde_json::json!({
            "code": "missing_structured_content",
            "tool": error.tool,
        });
        rmcp::ErrorData::internal_error(error.to_string(), Some(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal reaches the client as an internal error whose
    /// data names the code and the tool.
    #[test]
    fn converts_to_an_internal_error_naming_the_tool() {
        let error = MissingStructuredContent {
            tool: "version".to_string(),
        };
        let message = error.to_string();
        let data: rmcp::ErrorData = error.into();
        assert_eq!(data.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(data.message, message);
        assert!(message.contains("`version`"), "{message}");
        assert_eq!(
            data.data,
            Some(serde_json::json!({
                "code": "missing_structured_content",
                "tool": "version",
            })),
        );
    }
}
