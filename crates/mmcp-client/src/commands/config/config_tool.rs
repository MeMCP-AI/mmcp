//! The `config` MCP tool: argument validation, execution and the wire form of the result and the errors.

use std::path::Path;

use mmcp_core::config::NoticeLayers;
use rmcp::ErrorData as McpError;
use serde_json::{Value, json};

use super::error_chain::message_with_causes;
use super::{
    ConfigArgsError, ConfigEnvironment, ConfigOpError, ConfigOutcome, ConfigToolArgs,
    ProjectLocation,
};

/// Run one `config` tool call and return its result object.
///
/// # Errors
/// An invalid-params error for arguments that are not an operation or a project that does not exist, an internal error for a file that cannot be read, written or kept out of git.
/// Each carries its stable `code`.
pub fn call_config_tool(
    environment: &ConfigEnvironment<'_>,
    working_directory: &Path,
    args: &ConfigToolArgs,
) -> Result<Value, McpError> {
    let command = args.command().map_err(args_error_to_mcp)?;
    let explicit_root = args.project_path().map_err(args_error_to_mcp)?;
    let location =
        ProjectLocation::find(explicit_root, working_directory).map_err(op_error_to_mcp)?;
    let outcome = command
        .execute(environment, &location)
        .map_err(op_error_to_mcp)?;
    Ok(outcome_to_json(&outcome))
}

fn outcome_to_json(outcome: &ConfigOutcome) -> Value {
    match outcome {
        ConfigOutcome::Read { key, resolution } => json!({
            "action": "get",
            "key": key.as_str(),
            "effective": resolution.effective.as_str(),
            "source": resolution.source.as_str(),
            "layers": layers_to_json(resolution.layers),
        }),
        ConfigOutcome::Written(written) => json!({
            "action": if written.value.is_some() { "set" } else { "unset" },
            "key": written.key.as_str(),
            "scope": written.scope.as_str(),
            "value": written.value.map(|value| value.as_str()),
            "changed": written.changed,
            "file": written.file.to_string_lossy(),
            "excluded_in": written.excluded_in.as_ref().map(|file| file.to_string_lossy()),
            "effective": written.resolution.effective.as_str(),
            "source": written.resolution.source.as_str(),
        }),
    }
}

fn layers_to_json(layers: NoticeLayers) -> Value {
    Value::Object(
        layers
            .entries()
            .into_iter()
            .map(|(source, value)| {
                (
                    source.as_str().to_owned(),
                    json!(value.map(|value| value.as_str())),
                )
            })
            .collect(),
    )
}

fn args_error_to_mcp(error: ConfigArgsError) -> McpError {
    McpError::invalid_params(error.to_string(), Some(json!({ "code": error.code() })))
}

fn op_error_to_mcp(error: ConfigOpError) -> McpError {
    let code = error.code();
    let message = message_with_causes(&error);
    match &error {
        ConfigOpError::ProjectRootRequired { scope, .. } => McpError::invalid_params(
            message,
            Some(json!({ "code": code, "scope": scope.as_str() })),
        ),
        _ => McpError::internal_error(message, Some(json!({ "code": code }))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rmcp::model::ErrorCode;

    use super::super::config_fixture::{
        ConfigFixture, SCRATCH_EXCLUDES_FILE, appends_to_scratch_excludes, fails_to_exclude,
    };
    use super::*;

    fn call(fixture: &ConfigFixture, args: Value) -> Result<Value, McpError> {
        call_config_tool(
            &fixture.environment(appends_to_scratch_excludes),
            &fixture.project,
            &serde_json::from_value(args).unwrap(),
        )
    }

    fn code_of(error: &McpError) -> &str {
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
            .unwrap()
    }

    fn keys_of(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn a_get_reports_the_effective_value_its_source_and_every_layer() {
        let fixture = ConfigFixture::new();
        call(
            &fixture,
            json!({"action": "set", "key": "notice.md.project", "value": "off", "scope": "project"}),
        )
        .unwrap();

        let result = call(
            &fixture,
            json!({"action": "get", "key": "notice.md.project"}),
        )
        .unwrap();

        assert_eq!(
            keys_of(&result),
            ["action", "effective", "key", "layers", "source"]
        );
        assert_eq!(result["action"], "get");
        assert_eq!(result["key"], "notice.md.project");
        assert_eq!(result["effective"], "off");
        assert_eq!(result["source"], "project");
        assert_eq!(
            result["layers"],
            json!({
                "local": null,
                "project": "off",
                "flag": null,
                "environment": null,
                "user": null
            })
        );
    }

    #[test]
    fn a_set_reports_the_file_the_change_and_the_excludes_file() {
        let fixture = ConfigFixture::new();

        let result = call(
            &fixture,
            json!({"action": "set", "key": "notice.md.project", "value": "off", "scope": "local"}),
        )
        .unwrap();

        assert_eq!(
            keys_of(&result),
            [
                "action",
                "changed",
                "effective",
                "excluded_in",
                "file",
                "key",
                "scope",
                "source",
                "value"
            ]
        );
        assert_eq!(result["action"], "set");
        assert_eq!(result["scope"], "local");
        assert_eq!(result["value"], "off");
        assert_eq!(result["changed"], true);
        assert_eq!(
            result["file"],
            fixture
                .project
                .join(".mmcp.local.toml")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(result["excluded_in"], SCRATCH_EXCLUDES_FILE);
        assert_eq!(result["effective"], "off");
        assert_eq!(result["source"], "local");
    }

    #[test]
    fn an_unset_reports_a_null_value_and_the_layer_that_decides_next() {
        let fixture = ConfigFixture::new();
        call(
            &fixture,
            json!({"action": "set", "key": "notice.md.user", "value": "off", "scope": "user"}),
        )
        .unwrap();
        call(
            &fixture,
            json!({"action": "set", "key": "notice.md.user", "value": "on", "scope": "project"}),
        )
        .unwrap();

        let result = call(
            &fixture,
            json!({"action": "unset", "key": "notice.md.user", "scope": "project"}),
        )
        .unwrap();
        let repeat = call(
            &fixture,
            json!({"action": "unset", "key": "notice.md.user", "scope": "project"}),
        )
        .unwrap();

        assert_eq!(result["action"], "unset");
        assert_eq!(result["value"], Value::Null);
        assert_eq!(result["changed"], true);
        assert_eq!(result["effective"], "off");
        assert_eq!(result["source"], "user");
        assert_eq!(repeat["changed"], false);
    }

    #[test]
    fn every_argument_error_is_invalid_params_with_its_own_code_and_text() {
        let fixture = ConfigFixture::new();
        let cases = [
            (
                json!({"action": "set", "key": "notice.md.user", "scope": "user"}),
                "value_required",
                "set needs a value.",
            ),
            (
                json!({"action": "get", "key": "notice.md.user", "value": "on"}),
                "value_not_allowed",
                "get takes no value.",
            ),
            (
                json!({"action": "unset", "key": "notice.md.user", "value": "on", "scope": "user"}),
                "value_not_allowed",
                "unset takes no value.",
            ),
            (
                json!({"action": "set", "key": "notice.md.user", "value": "on"}),
                "scope_required",
                "set needs a scope.",
            ),
            (
                json!({"action": "unset", "key": "notice.md.user"}),
                "scope_required",
                "unset needs a scope.",
            ),
            (
                json!({"action": "get", "key": "notice.md.user", "scope": "user"}),
                "scope_not_allowed",
                "get takes no scope.",
            ),
        ];
        for (args, code, message) in cases {
            let error = call(&fixture, args.clone()).unwrap_err();
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS, "{args}");
            assert_eq!(code_of(&error), code, "{args}");
            assert_eq!(error.message, message, "{args}");
        }
    }

    #[test]
    fn a_project_scope_without_a_project_is_project_not_found_naming_the_directory_and_scope() {
        let fixture = ConfigFixture::new();
        let bare = fixture.project.join("bare");
        std::fs::create_dir_all(&bare).unwrap();

        for scope in ["project", "local"] {
            let error = call(
                &fixture,
                json!({
                    "action": "set", "key": "notice.md.project", "value": "off",
                    "scope": scope, "path": bare.to_string_lossy()
                }),
            )
            .unwrap_err();

            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert_eq!(code_of(&error), "project_not_found");
            assert_eq!(
                error.message,
                format!(
                    "No mmcp project at {}. Scope {scope} needs a .mmcp.toml.",
                    bare.display()
                )
            );
            assert_eq!(error.data.as_ref().unwrap()["scope"], scope);
        }
    }

    #[test]
    fn the_user_scope_and_a_get_work_without_a_project() {
        let fixture = ConfigFixture::new();
        let bare = fixture.project.join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        let path = bare.to_string_lossy().into_owned();

        let set = call(
            &fixture,
            json!({
                "action": "set", "key": "notice.md.user", "value": "off",
                "scope": "user", "path": path
            }),
        )
        .unwrap();
        let get = call(
            &fixture,
            json!({"action": "get", "key": "notice.md.user", "path": path}),
        )
        .unwrap();

        assert_eq!(set["changed"], true);
        assert_eq!(get["effective"], "off");
        assert_eq!(get["layers"]["project"], Value::Null);
        assert_eq!(get["layers"]["local"], Value::Null);
    }

    #[test]
    fn a_file_that_fails_to_load_is_an_internal_error_with_its_code_and_its_cause() {
        let fixture = ConfigFixture::new();
        std::fs::write(fixture.project.join(".mmcp.toml"), "not = [valid").unwrap();

        let error = call(&fixture, json!({"action": "get", "key": "notice.md.user"})).unwrap_err();

        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(code_of(&error), "project_config_load_failed");
        assert!(
            error
                .message
                .starts_with("The project config could not be loaded: "),
            "the cause follows the sentence without its period: {}",
            error.message
        );
        assert!(
            error.message.len() > "The project config could not be loaded: ".len(),
            "{}",
            error.message
        );
        assert!(!error.message.contains(".:"), "{}", error.message);
    }

    #[test]
    fn an_empty_or_over_long_path_is_refused_before_it_reaches_the_file_system() {
        let fixture = ConfigFixture::new();
        let too_long = "p".repeat(super::super::config_tool_args::MAX_PATH_CHARS + 1);
        let cases = [
            (json!(""), "path_empty", "path is empty.".to_owned()),
            (
                json!(too_long),
                "path_too_long",
                format!(
                    "path is longer than {} characters.",
                    super::super::config_tool_args::MAX_PATH_CHARS
                ),
            ),
        ];
        for (path, code, message) in cases {
            let error = call(
                &fixture,
                json!({"action": "get", "key": "notice.md.user", "path": path}),
            )
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert_eq!(code_of(&error), code);
            assert_eq!(error.message, message);
        }
    }

    #[test]
    fn a_tracked_local_file_is_an_internal_error_with_its_code_and_nothing_is_written() {
        let fixture = ConfigFixture::new();
        let tracked = fixture.environment(super::super::config_fixture::already_tracked);

        let error = call_config_tool(
            &tracked,
            &fixture.project,
            &serde_json::from_value(json!({
                "action": "set", "key": "notice.md.project", "value": "off", "scope": "local"
            }))
            .unwrap(),
        )
        .unwrap_err();

        assert_eq!(code_of(&error), "local_config_tracked");
        assert!(!fixture.project.join(".mmcp.local.toml").exists());
    }

    #[test]
    fn a_failing_exclusion_is_an_internal_error_and_writes_nothing() {
        let fixture = ConfigFixture::new();
        let failing = fixture.environment(fails_to_exclude);

        let error = call_config_tool(
            &failing,
            &fixture.project,
            &serde_json::from_value(json!({
                "action": "set", "key": "notice.md.project", "value": "off", "scope": "local"
            }))
            .unwrap(),
        )
        .unwrap_err();

        assert_eq!(code_of(&error), "local_config_exclude_failed");
        assert!(!fixture.project.join(".mmcp.local.toml").exists());
    }
}
