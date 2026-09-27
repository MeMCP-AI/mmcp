//! Bringing a served tool result in line with its tool's output schema.

use rmcp::model::{CallToolResponse, CallToolResult};

use super::MissingStructuredContent;

/// Bring a served tool response in line with the output schema its
/// tool declares.
///
/// A tool declaring no output schema passes through untouched, and so
/// does a response other than a final result. A result already carrying
/// structured content is kept as is. An error result without structured
/// content, such as rmcp's argument-error result, receives
/// `{"error": <its text>}`. A successful result without structured
/// content is refused with [`MissingStructuredContent`].
pub(crate) fn conform_to_output_schema(
    tool: &str,
    declares_output_schema: bool,
    response: CallToolResponse,
) -> Result<CallToolResponse, MissingStructuredContent> {
    if !declares_output_schema {
        return Ok(response);
    }
    match response {
        CallToolResponse::Complete(result) => {
            conform_result(tool, result).map(CallToolResponse::Complete)
        }
        other => Ok(other),
    }
}

/// Conform one final result; see [`conform_to_output_schema`].
fn conform_result(
    tool: &str,
    mut result: CallToolResult,
) -> Result<CallToolResult, MissingStructuredContent> {
    if result.structured_content.is_some() {
        return Ok(result);
    }
    if result.is_error != Some(true) {
        return Err(MissingStructuredContent {
            tool: tool.to_string(),
        });
    }
    let text = result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    result.structured_content = Some(serde_json::json!({ "error": text }));
    Ok(result)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rmcp::model::ContentBlock;
    use serde_json::json;

    fn complete(response: CallToolResponse) -> CallToolResult {
        match response {
            CallToolResponse::Complete(result) => result,
            other => panic!("expected a final result, got {other:?}"),
        }
    }

    /// A result the helpers built keeps its structured content and
    /// its text byte for byte.
    #[test]
    fn keeps_a_structured_success_result() {
        let value = json!({"package_version": "1.2.3"});
        let result = CallToolResult::structured(value.clone());
        let conformed =
            complete(conform_to_output_schema("version", true, result.into()).expect("conforms"));
        assert_eq!(conformed.structured_content, Some(value.clone()));
        assert_eq!(conformed.is_error, Some(false));
        assert_eq!(
            conformed.content[0].as_text().expect("text").text,
            value.to_string()
        );
    }

    /// A successful result without structured content is refused,
    /// naming the tool, never patched.
    #[test]
    fn refuses_a_success_result_without_structured_content() {
        let result = CallToolResult::success(vec![ContentBlock::text("{}")]);
        let refused = conform_to_output_schema("list_groups", true, result.into())
            .expect_err("a bypassed helper must be refused");
        assert_eq!(
            refused,
            MissingStructuredContent {
                tool: "list_groups".to_string()
            }
        );
    }

    /// A result with no `is_error` flag is a success, so it is
    /// refused the same way when it lacks structured content.
    #[test]
    fn refuses_an_unflagged_result_without_structured_content() {
        let mut result = CallToolResult::success(vec![ContentBlock::text("{}")]);
        result.is_error = None;
        let refused = conform_to_output_schema("status", true, result.into())
            .expect_err("an unflagged result is a success");
        assert_eq!(refused.tool, "status");
    }

    /// An error result without structured content receives an object
    /// carrying its text, every text block joined, content untouched.
    #[test]
    fn gives_an_error_result_its_text_as_structured_error() {
        let result = CallToolResult::error(vec![
            ContentBlock::text("failed to deserialize parameters: unknown field"),
            ContentBlock::text("second line"),
        ]);
        let conformed =
            complete(conform_to_output_schema("version", true, result.into()).expect("conforms"));
        assert_eq!(conformed.is_error, Some(true));
        assert_eq!(
            conformed.structured_content,
            Some(json!({
                "error": "failed to deserialize parameters: unknown field\nsecond line",
            })),
        );
        assert_eq!(conformed.content.len(), 2);
    }

    /// An error result already carrying structured content is kept.
    #[test]
    fn keeps_a_structured_error_result() {
        let value = json!({"error": "custom", "code": "custom_code"});
        let result = CallToolResult::structured_error(value.clone());
        let conformed =
            complete(conform_to_output_schema("version", true, result.into()).expect("conforms"));
        assert_eq!(conformed.structured_content, Some(value));
    }

    /// A tool declaring no output schema owes no structured content,
    /// so its result passes through untouched.
    #[test]
    fn leaves_a_tool_without_output_schema_untouched() {
        let result = CallToolResult::success(vec![ContentBlock::text("plain")]);
        let conformed =
            complete(conform_to_output_schema("plain", false, result.into()).expect("passes"));
        assert_eq!(conformed.structured_content, None);
        assert_eq!(conformed.content[0].as_text().expect("text").text, "plain");
    }
}
