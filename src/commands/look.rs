use std::env;

use serde_json::{Map, Value, json};

use crate::{
    configuration::load_bundle_configuration,
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        association::WorkspaceContext, error::RuntimeError, paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{LookArguments, RuntimeArguments, shared};

pub(super) fn run_agentmux_look(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_look_help();
        return Ok(());
    }

    let parsed = parse_look_arguments(arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, None)?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let resolved_session = resolve_tui_session_identity(
        &roots.configuration_root,
        &workspace.workspace_root,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    load_bundle_configuration(&roots.configuration_root, &resolved_session.bundle_name)
        .map_err(shared::map_bundle_load_error)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &resolved_session.bundle_name)?;
    let response = request_relay(
        &paths.relay_socket,
        &RelayRequest::Look {
            requester_session: resolved_session.session_id,
            target_session: parsed.target_session,
            lines: parsed.lines.map(|value| value as usize),
            bundle_name: Some(resolved_session.bundle_name),
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::Look {
            schema_version,
            bundle_name,
            requester_session,
            target_session,
            captured_at,
            snapshot_lines,
            freshness,
            snapshot_source,
            stale_reason_code,
            snapshot_age_ms,
        } => {
            let mut payload = Map::new();
            payload.insert("schema_version".to_string(), Value::String(schema_version));
            payload.insert("bundle_name".to_string(), Value::String(bundle_name));
            payload.insert(
                "requester_session".to_string(),
                Value::String(requester_session),
            );
            payload.insert("target_session".to_string(), Value::String(target_session));
            payload.insert("captured_at".to_string(), Value::String(captured_at));
            payload.insert("snapshot_lines".to_string(), json!(snapshot_lines));
            if let Some(value) = freshness {
                payload.insert("freshness".to_string(), json!(value));
            }
            if let Some(value) = snapshot_source {
                payload.insert("snapshot_source".to_string(), json!(value));
            }
            if let Some(value) = stale_reason_code {
                payload.insert("stale_reason_code".to_string(), Value::String(value));
            }
            if let Some(value) = snapshot_age_ms {
                payload.insert("snapshot_age_ms".to_string(), json!(value));
            }
            Value::Object(payload)
        }
        RelayResponse::Error { error } => return Err(shared::map_relay_error(error)),
        other => {
            return Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            ));
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).map_err(|source| {
            RuntimeError::io("encode look response json", std::io::Error::other(source))
        })?
    );
    Ok(())
}

fn parse_look_arguments(arguments: &[String]) -> Result<LookArguments, RuntimeError> {
    let mut parsed = LookArguments {
        bundle_name: None,
        session_selector: None,
        target_session: String::new(),
        lines: None,
        runtime: RuntimeArguments::default(),
    };
    let mut target_session = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--as-session" => {
                parsed.session_selector =
                    Some(shared::take_value(arguments, &mut index, "--as-session")?);
            }
            "--lines" => {
                let value = shared::take_value(arguments, &mut index, "--lines")?;
                parsed.lines = Some(shared::parse_look_lines(value.as_str())?);
            }
            value if !value.starts_with('-') => {
                if target_session.is_some() {
                    return Err(RuntimeError::InvalidArgument {
                        argument: value.to_string(),
                        message: "unknown argument".to_string(),
                    });
                }
                target_session = Some(value.to_string());
            }
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    parsed.target_session = target_session.ok_or_else(|| RuntimeError::InvalidArgument {
        argument: "<target-session>".to_string(),
        message: "missing value".to_string(),
    })?;
    Ok(parsed)
}

pub(super) fn print_look_help() {
    println!(
        "Usage: agentmux look <target-session> [--bundle NAME] [--as-session NAME] [--lines N] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
