use std::env;

use serde_json::{Map, Value, json};

use crate::{
    configuration::load_bundle_configuration,
    relay::{ListedSessionTransport, RelayRequest, RelayResponse, request_relay},
    runtime::{
        association::WorkspaceContext, error::RuntimeError, paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{RawwArguments, RuntimeArguments, shared};

pub(super) fn run_agentmux_raww(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_raww_help();
        return Ok(());
    }

    let parsed = parse_raww_arguments(arguments)?;
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
        &RelayRequest::Raww {
            request_id: None,
            sender_session: resolved_session.session_id,
            target_session: parsed.target_session,
            text: parsed.text,
            no_enter: parsed.no_enter,
            bundle_name: Some(resolved_session.bundle_name),
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::Raww {
            schema_version,
            status,
            target_session,
            transport,
            request_id,
            message_id,
            details,
        } => {
            let mut payload = Map::new();
            payload.insert("schema_version".to_string(), Value::String(schema_version));
            payload.insert("status".to_string(), Value::String(status));
            payload.insert("target_session".to_string(), Value::String(target_session));
            payload.insert("transport".to_string(), json!(render_transport(&transport)));
            if let Some(value) = request_id {
                payload.insert("request_id".to_string(), Value::String(value));
            }
            if let Some(value) = message_id {
                payload.insert("message_id".to_string(), Value::String(value));
            }
            if let Some(value) = details {
                payload.insert("details".to_string(), value);
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

    if parsed.output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|source| {
                RuntimeError::io("encode raww response json", std::io::Error::other(source))
            })?
        );
    } else {
        let status = payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target_session = payload
            .get("target_session")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let transport = payload
            .get("transport")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let delivery_phase = payload
            .get("details")
            .and_then(|details| details.get("delivery_phase"))
            .and_then(Value::as_str);
        if let Some(delivery_phase) = delivery_phase {
            println!(
                "status={status} target={target_session} transport={transport} phase={delivery_phase}"
            );
        } else {
            println!("status={status} target={target_session} transport={transport}");
        }
    }
    Ok(())
}

fn parse_raww_arguments(arguments: &[String]) -> Result<RawwArguments, RuntimeError> {
    let mut parsed = RawwArguments {
        bundle_name: None,
        session_selector: None,
        target_session: String::new(),
        text: String::new(),
        no_enter: false,
        output_json: false,
        runtime: RuntimeArguments::default(),
    };
    let mut target_session = None::<String>;
    let mut text = None::<String>;
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
            "--text" => {
                text = Some(shared::take_value(arguments, &mut index, "--text")?);
            }
            "--no-enter" => parsed.no_enter = true,
            "--json" => parsed.output_json = true,
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
    parsed.text = text.ok_or_else(|| {
        RuntimeError::validation(
            "validation_invalid_params",
            "text is required via --text".to_string(),
        )
    })?;
    Ok(parsed)
}

fn render_transport(transport: &ListedSessionTransport) -> &'static str {
    match transport {
        ListedSessionTransport::Tmux => "tmux",
        ListedSessionTransport::Acp => "acp",
    }
}

pub(super) fn print_raww_help() {
    println!(
        "Usage: agentmux raww <target-session> --text TEXT [--no-enter] [--bundle NAME] [--as-session NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
