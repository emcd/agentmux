use rmcp::ErrorData as McpError;
use serde_json::json;

use super::errors::validation_tool_error;
use super::params::{
    GRANT_OUTCOME_CANCELLED, GRANT_OUTCOME_SELECTED, GrantListArgs, GrantResolveArgs,
    LIST_COMMAND_SESSIONS, LOOK_LINES_MAX, LOOK_LINES_MIN, ListArgs, LookParams, RawwParams,
    SendParams,
};

pub(super) fn validate_list_request(
    command: Option<&str>,
    args: &ListArgs,
) -> Result<(), McpError> {
    let command = command
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation_tool_error(
                "validation_invalid_params",
                "command is required and must equal \"sessions\"",
                None,
            )
        })?;
    if command != LIST_COMMAND_SESSIONS {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "command is required and must equal \"sessions\"",
            Some(json!({"command": command})),
        ));
    }
    if args.all && args.bundle_name.is_some() {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name and all=true are mutually exclusive",
            None,
        ));
    }
    if let Some(bundle_name) = args.bundle_name.as_ref()
        && bundle_name.trim().is_empty()
    {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name must be non-empty when provided",
            None,
        ));
    }
    Ok(())
}

pub(super) fn parse_meta_tool_args<T: serde::de::DeserializeOwned + Default>(
    value: serde_json::Value,
) -> Result<T, String> {
    if value.is_null() {
        return Ok(T::default());
    }
    let value = match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return Ok(T::default());
            }
            serde_json::Value::Object(map)
        }
        other => {
            return Err(format!(
                "args must be a JSON object, got {}",
                json_type_name(&other)
            ));
        }
    };
    serde_json::from_value::<T>(value).map_err(|err| err.to_string())
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub(super) fn validate_send_request(params: &SendParams) -> Result<(), McpError> {
    let message = params.message.trim();
    if message.is_empty() {
        return Err(validation_tool_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if params.broadcast && !params.targets.is_empty() {
        return Err(validation_tool_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if !params.broadcast && params.targets.is_empty() {
        return Err(validation_tool_error(
            "validation_empty_targets",
            "provide at least one target or set broadcast=true",
            None,
        ));
    }
    if matches!(params.quiescence_timeout_ms, Some(0)) {
        return Err(validation_tool_error(
            "validation_invalid_quiescence_timeout",
            "quiescence_timeout_ms must be greater than zero milliseconds",
            None,
        ));
    }
    if matches!(params.acp_turn_timeout_ms, Some(0)) {
        return Err(validation_tool_error(
            "validation_invalid_acp_turn_timeout",
            "acp_turn_timeout_ms must be greater than zero milliseconds",
            None,
        ));
    }
    if params.quiescence_timeout_ms.is_some() && params.acp_turn_timeout_ms.is_some() {
        return Err(validation_tool_error(
            "validation_conflicting_timeout_fields",
            "quiescence_timeout_ms and acp_turn_timeout_ms are mutually exclusive",
            None,
        ));
    }
    Ok(())
}

pub(super) fn validate_look_request(params: &LookParams) -> Result<(), McpError> {
    if params.target_session.trim().is_empty() {
        return Err(validation_tool_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    if let Some(lines) = params.lines
        && !(LOOK_LINES_MIN..=LOOK_LINES_MAX).contains(&lines)
    {
        return Err(validation_tool_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": lines,
                "min": LOOK_LINES_MIN,
                "max": LOOK_LINES_MAX,
            })),
        ));
    }
    Ok(())
}

pub(super) fn validate_raww_request(params: &RawwParams) -> Result<(), McpError> {
    if params.target_session.trim().is_empty() {
        return Err(validation_tool_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    let mut provided_fields = params.extra_fields.keys().cloned().collect::<Vec<_>>();
    provided_fields.sort();
    if !provided_fields.is_empty() {
        let sender_like = provided_fields
            .iter()
            .filter(|field| is_sender_like_field(field.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !sender_like.is_empty() {
            return Err(validation_tool_error(
                "validation_invalid_params",
                "sender-like fields are not allowed; sender authority is association-derived",
                Some(json!({"fields": sender_like})),
            ));
        }
        return Err(validation_tool_error(
            "validation_invalid_params",
            "unknown parameter(s) for raww request",
            Some(json!({"fields": provided_fields})),
        ));
    }
    Ok(())
}

fn is_sender_like_field(field: &str) -> bool {
    matches!(
        field,
        "sender"
            | "sender_id"
            | "sender_name"
            | "sender_session"
            | "sender_session_id"
            | "requester"
            | "requester_session"
            | "requester_session_id"
            | "as_session"
    )
}

fn is_grant_decider_field(field: &str) -> bool {
    matches!(
        field,
        "decided_by" | "ui_session_id" | "operator_session_id" | "decider" | "decider_session"
    )
}

pub(super) fn validate_grant_list_args(args: &GrantListArgs) -> Result<(), McpError> {
    if let Some(bundle_name) = args.bundle_name.as_ref()
        && bundle_name.trim().is_empty()
    {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name must be non-empty when provided",
            None,
        ));
    }
    let mut provided_fields = args.extra_fields.keys().cloned().collect::<Vec<_>>();
    provided_fields.sort();
    if !provided_fields.is_empty() {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "unknown parameter(s) for grant list command",
            Some(json!({"fields": provided_fields})),
        ));
    }
    Ok(())
}

pub(super) fn validate_grant_resolve_args(args: &GrantResolveArgs) -> Result<(), McpError> {
    let permission_request_id = args
        .permission_request_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation_tool_error(
                "validation_invalid_params",
                "permission_request_id must be a non-empty string",
                Some(json!({"field": "permission_request_id"})),
            )
        })?;
    let _ = permission_request_id;
    let outcome = args
        .outcome
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation_tool_error(
                "validation_invalid_params",
                "outcome must be \"selected\" or \"cancelled\"",
                Some(json!({"field": "outcome"})),
            )
        })?;
    match outcome {
        GRANT_OUTCOME_SELECTED => {
            let option_id = args
                .option_id
                .as_ref()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty());
            if option_id.is_none() {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "selected outcome requires explicit non-empty option_id",
                    Some(json!({
                        "field": "option_id",
                        "outcome": GRANT_OUTCOME_SELECTED,
                    })),
                ));
            }
        }
        GRANT_OUTCOME_CANCELLED => {
            if args.option_id.is_some() {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "cancelled outcome must omit option_id",
                    Some(json!({
                        "field": "option_id",
                        "outcome": GRANT_OUTCOME_CANCELLED,
                    })),
                ));
            }
        }
        other => {
            return Err(validation_tool_error(
                "validation_invalid_params",
                "outcome must be \"selected\" or \"cancelled\"",
                Some(json!({"field": "outcome", "value": other})),
            ));
        }
    }
    if let Some(bundle_name) = args.bundle_name.as_ref()
        && bundle_name.trim().is_empty()
    {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name must be non-empty when provided",
            None,
        ));
    }
    let mut provided_fields = args.extra_fields.keys().cloned().collect::<Vec<_>>();
    provided_fields.sort();
    if !provided_fields.is_empty() {
        let decider_like = provided_fields
            .iter()
            .filter(|field| is_grant_decider_field(field.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !decider_like.is_empty() {
            return Err(validation_tool_error(
                "validation_invalid_params",
                "decider-identity fields are not allowed; decided_by is association-derived",
                Some(json!({"fields": decider_like})),
            ));
        }
        return Err(validation_tool_error(
            "validation_invalid_params",
            "unknown parameter(s) for grant resolve command",
            Some(json!({"fields": provided_fields})),
        ));
    }
    Ok(())
}

pub(super) fn is_relay_unavailable_error(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

pub(super) fn is_relay_timeout_error(source: &std::io::Error) -> bool {
    matches!(source.kind(), std::io::ErrorKind::TimedOut)
}
