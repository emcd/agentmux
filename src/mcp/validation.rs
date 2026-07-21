use std::collections::BTreeMap;

use rmcp::ErrorData as McpError;
use serde_json::json;

use super::errors::validation_tool_error;
use super::params::{
    CHOOSE_OUTCOME_CANCELLED, CHOOSE_OUTCOME_SELECTED, ChangeParams, ChangePskArgs, ChooseParams,
    HelpParams, LOOK_LINES_MAX, LOOK_LINES_MIN, ListArgs, ListDecisionsArgs, ListNamespacesArgs,
    ListParams, ListRelaysArgs, LookParams, NewParams, NewPeerArgs, RawwParams, SendParams,
    UpdownArgs, UpdownParams,
};

pub(super) fn validate_list_params(params: &ListParams) -> Result<(), McpError> {
    validate_unknown_fields("list request", None, &params.extra_fields)
}

pub(super) fn validate_help_request(params: &HelpParams) -> Result<(), McpError> {
    validate_unknown_fields("help request", None, &params.extra_fields)
}

pub(super) fn validate_updown_params(params: &UpdownParams) -> Result<(), McpError> {
    validate_unknown_fields("updown request", None, &params.extra_fields)
}

pub(super) fn validate_updown_args(args: &UpdownArgs, command: &str) -> Result<(), McpError> {
    let context = format!("updown {command} command");
    validate_unknown_fields(context.as_str(), Some("args"), &args.extra_fields)
}

pub(super) fn validate_new_params(params: &NewParams) -> Result<(), McpError> {
    validate_unknown_fields("new request", None, &params.extra_fields)
}

pub(super) fn validate_new_peer_args(args: &NewPeerArgs) -> Result<(), McpError> {
    validate_unknown_fields("new peer command", Some("args"), &args.extra_fields)
}

pub(super) fn validate_change_params(params: &ChangeParams) -> Result<(), McpError> {
    validate_unknown_fields("change request", None, &params.extra_fields)
}

pub(super) fn validate_change_psk_args(args: &ChangePskArgs) -> Result<(), McpError> {
    validate_unknown_fields("change psk command", Some("args"), &args.extra_fields)
}

pub(super) fn validate_list_principals_args(args: &ListArgs) -> Result<(), McpError> {
    validate_unknown_fields("list principals command", Some("args"), &args.extra_fields)?;
    let namespace = args
        .namespace
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match args.relay.as_deref().map(str::trim) {
        // Foreign principal discovery: a concrete namespace is required and the
        // relay selector must name a configured peer alias.
        Some(relay) => {
            if relay.is_empty() || relay == "*" {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "relay must be a configured peer alias, not empty or \"*\"",
                    Some(json!({"relay": args.relay})),
                ));
            }
            let Some(namespace) = namespace else {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "namespace is required and must name one concrete namespace when relay is set",
                    Some(json!({"relay": relay})),
                ));
            };
            if namespace == "*" || matches!(namespace, "ALL" | "EXTERNAL" | "RELAY") {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "foreign namespace must name one concrete namespace, not \"*\" or a reserved token",
                    Some(json!({"namespace": namespace})),
                ));
            }
            Ok(())
        }
        // Local principal listing keeps its existing selector semantics.
        None => {
            if let Some(namespace) = namespace
                && matches!(namespace, "ALL" | "EXTERNAL" | "RELAY")
            {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "namespace must be a bundle name, \"GLOBAL\", or \"*\"",
                    Some(json!({"namespace": namespace})),
                ));
            }
            Ok(())
        }
    }
}

pub(super) fn validate_list_namespaces_args(args: &ListNamespacesArgs) -> Result<(), McpError> {
    validate_unknown_fields("list namespaces command", Some("args"), &args.extra_fields)?;
    if let Some(relay) = args.relay.as_deref().map(str::trim)
        && (relay.is_empty() || relay == "*")
    {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "relay must be a configured peer alias, not empty or \"*\"",
            Some(json!({"relay": args.relay})),
        ));
    }
    Ok(())
}

pub(super) fn validate_list_relays_args(args: &ListRelaysArgs) -> Result<(), McpError> {
    validate_unknown_fields("list relays command", Some("args"), &args.extra_fields)
}

pub(super) fn validate_list_decisions_args(args: &ListDecisionsArgs) -> Result<(), McpError> {
    validate_unknown_fields("list decisions command", Some("args"), &args.extra_fields)
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
    validate_unknown_fields("send request", None, &params.extra_fields)?;
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
    Ok(())
}

/// Qualifies one caller-supplied target with the namespace the relay requires on
/// every target. A target that already carries an `@<namespace>` suffix passes
/// through verbatim (so a `<session>@<peer-bundle>` target still reaches a peer
/// bundle); a bare target is qualified with the MCP server's bound bundle. A bare
/// target on a relay-wide (unassociated) MCP server has no namespace to borrow and
/// is rejected as `validation_unqualified_target` rather than silently borrowing
/// one. Shared by `send`, `look`, and `raww` so bare-target ergonomics are uniform
/// across the delivery/inspection surface.
pub(super) fn qualify_target(target: &str, bound_bundle: Option<&str>) -> Result<String, McpError> {
    if target.contains('@') {
        Ok(target.to_string())
    } else if let Some(bundle) = bound_bundle {
        Ok(format!("{target}@{bundle}"))
    } else {
        Err(validation_tool_error(
            "validation_unqualified_target",
            "target must be a fully-qualified principal id (id@namespace); \
             a relay-wide MCP server cannot infer a bundle namespace",
            Some(json!({"target": target})),
        ))
    }
}

pub(super) fn qualify_send_targets(
    targets: &[String],
    bound_bundle: Option<&str>,
) -> Result<Vec<String>, McpError> {
    targets
        .iter()
        .map(|target| qualify_target(target, bound_bundle))
        .collect()
}

pub(super) fn validate_look_request(params: &LookParams) -> Result<(), McpError> {
    validate_unknown_fields("look request", None, &params.extra_fields)?;
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
    validate_unknown_fields("raww request", None, &params.extra_fields)?;
    if params.target_session.trim().is_empty() {
        return Err(validation_tool_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    Ok(())
}

pub(super) fn validate_choose_request(params: &ChooseParams) -> Result<(), McpError> {
    validate_unknown_fields("choose request", None, &params.extra_fields)?;
    let choice_request_id = params
        .choice_request_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation_tool_error(
                "validation_invalid_params",
                "choice_request_id must be a non-empty string",
                Some(json!({"field": "choice_request_id"})),
            )
        })?;
    let _ = choice_request_id;
    let outcome = params
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
        CHOOSE_OUTCOME_SELECTED => {
            let option_id = params
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
                        "outcome": CHOOSE_OUTCOME_SELECTED,
                    })),
                ));
            }
        }
        CHOOSE_OUTCOME_CANCELLED => {
            if params.option_id.is_some() {
                return Err(validation_tool_error(
                    "validation_invalid_params",
                    "cancelled outcome must omit option_id",
                    Some(json!({
                        "field": "option_id",
                        "outcome": CHOOSE_OUTCOME_CANCELLED,
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
    Ok(())
}

fn validate_unknown_fields(
    context: &str,
    prefix: Option<&str>,
    extra_fields: &BTreeMap<String, serde_json::Value>,
) -> Result<(), McpError> {
    if extra_fields.is_empty() {
        return Ok(());
    }
    let fields = extra_fields
        .keys()
        .map(|field| match prefix {
            Some(prefix) => format!("{prefix}.{field}"),
            None => field.clone(),
        })
        .collect::<Vec<_>>();
    let message = format!("unknown parameter(s) for {context}: {}", fields.join(", "));
    Err(validation_tool_error(
        "validation_invalid_params",
        message.as_str(),
        Some(json!({"fields": fields})),
    ))
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
