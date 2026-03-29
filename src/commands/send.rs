use std::{
    env,
    io::{IsTerminal, Read},
};

use serde_json::json;

use crate::{
    configuration::load_bundle_configuration,
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        association::WorkspaceContext, error::RuntimeError, paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{RuntimeArguments, SendArguments, shared};

pub(super) fn run_agentmux_send(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_send_help();
        return Ok(());
    }

    let parsed = parse_send_arguments(arguments)?;
    validate_send_targets(&parsed)?;
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
        &RelayRequest::Chat {
            request_id: parsed.request_id.clone(),
            sender_session: resolved_session.session_id,
            message: parsed.message.clone(),
            targets: parsed.targets.clone(),
            broadcast: parsed.broadcast,
            delivery_mode: parsed.delivery_mode,
            quiet_window_ms: None,
            quiescence_timeout_ms: parsed.quiescence_timeout_ms,
            acp_turn_timeout_ms: parsed.acp_turn_timeout_ms,
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::Chat {
            schema_version,
            bundle_name,
            request_id,
            sender_session,
            sender_display_name,
            delivery_mode,
            status,
            results,
        } => json!({
            "schema_version": schema_version,
            "bundle_name": bundle_name,
            "request_id": request_id,
            "sender_session": sender_session,
            "sender_display_name": sender_display_name,
            "delivery_mode": delivery_mode,
            "status": status,
            "results": results,
        }),
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
                RuntimeError::io("encode send response json", std::io::Error::other(source))
            })?
        );
    } else {
        println!(
            "bundle={} mode={:?} status={}",
            payload["bundle_name"].as_str().unwrap_or_default(),
            parsed.delivery_mode,
            payload["status"].as_str().unwrap_or_default(),
        );
        if let Some(results) = payload["results"].as_array() {
            for result in results {
                let target = result["target_session"].as_str().unwrap_or_default();
                let outcome = result["outcome"].as_str().unwrap_or_default();
                if let Some(reason) = result["reason"].as_str() {
                    println!("{target}\t{outcome}\t{reason}");
                } else {
                    println!("{target}\t{outcome}");
                }
            }
        }
    }
    Ok(())
}

fn parse_send_arguments(arguments: &[String]) -> Result<SendArguments, RuntimeError> {
    let mut bundle_name = None;
    let mut session_selector = None;
    let mut request_id = None;
    let mut targets = Vec::<String>::new();
    let mut broadcast = false;
    let mut message = None;
    let mut delivery_mode = crate::relay::ChatDeliveryMode::Async;
    let mut quiescence_timeout_ms = None;
    let mut acp_turn_timeout_ms = None;
    let mut output_json = false;
    let mut runtime = RuntimeArguments::default();
    let mut index = 0usize;

    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--session" => {
                session_selector = Some(shared::take_value(arguments, &mut index, "--session")?);
            }
            "--request-id" => {
                request_id = Some(shared::take_value(arguments, &mut index, "--request-id")?);
            }
            "--target" => targets.push(shared::take_value(arguments, &mut index, "--target")?),
            "--broadcast" => broadcast = true,
            "--message" => message = Some(shared::take_value(arguments, &mut index, "--message")?),
            "--delivery-mode" => {
                let value = shared::take_value(arguments, &mut index, "--delivery-mode")?;
                delivery_mode = shared::parse_delivery_mode(value.as_str())?;
            }
            "--quiescence-timeout-ms" => {
                let value = shared::take_value(arguments, &mut index, "--quiescence-timeout-ms")?;
                quiescence_timeout_ms = Some(shared::parse_positive_u64(
                    value.as_str(),
                    "--quiescence-timeout-ms",
                    "validation_invalid_quiescence_timeout",
                    "quiescence timeout override must be greater than zero milliseconds",
                )?);
            }
            "--acp-turn-timeout-ms" => {
                let value = shared::take_value(arguments, &mut index, "--acp-turn-timeout-ms")?;
                acp_turn_timeout_ms = Some(shared::parse_positive_u64(
                    value.as_str(),
                    "--acp-turn-timeout-ms",
                    "validation_invalid_acp_turn_timeout",
                    "ACP turn timeout override must be greater than zero milliseconds",
                )?);
            }
            "--json" => output_json = true,
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }

    let message = resolve_send_message(message)?;
    if message.trim().is_empty() {
        return Err(RuntimeError::validation(
            "validation_invalid_arguments",
            "message must be non-empty".to_string(),
        ));
    }
    Ok(SendArguments {
        bundle_name,
        session_selector,
        request_id,
        message,
        targets,
        broadcast,
        delivery_mode,
        quiescence_timeout_ms,
        acp_turn_timeout_ms,
        output_json,
        runtime,
    })
}

fn resolve_send_message(message_flag: Option<String>) -> Result<String, RuntimeError> {
    let stdin_is_terminal = std::io::stdin().is_terminal();
    if let Some(message) = message_flag {
        if !stdin_is_terminal {
            return Err(RuntimeError::validation(
                "validation_conflicting_message_input",
                "provide either --message or piped stdin, not both".to_string(),
            ));
        }
        return Ok(message);
    }
    if stdin_is_terminal {
        return Err(RuntimeError::validation(
            "validation_missing_message_input",
            "message input is required via --message or piped stdin".to_string(),
        ));
    }
    let mut buffer = String::new();
    std::io::stdin()
        .read_to_string(&mut buffer)
        .map_err(|source| RuntimeError::io("read send message from stdin", source))?;
    if buffer.trim().is_empty() {
        return Err(RuntimeError::validation(
            "validation_missing_message_input",
            "message input is required via --message or piped stdin".to_string(),
        ));
    }
    Ok(buffer)
}

fn validate_send_targets(arguments: &SendArguments) -> Result<(), RuntimeError> {
    if arguments.broadcast && !arguments.targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true".to_string(),
        ));
    }
    if !arguments.broadcast && arguments.targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_empty_targets",
            "provide at least one --target or set --broadcast".to_string(),
        ));
    }
    if matches!(arguments.quiescence_timeout_ms, Some(0)) {
        return Err(RuntimeError::validation(
            "validation_invalid_quiescence_timeout",
            "quiescence timeout override must be greater than zero milliseconds".to_string(),
        ));
    }
    if matches!(arguments.acp_turn_timeout_ms, Some(0)) {
        return Err(RuntimeError::validation(
            "validation_invalid_acp_turn_timeout",
            "ACP turn timeout override must be greater than zero milliseconds".to_string(),
        ));
    }
    if arguments.quiescence_timeout_ms.is_some() && arguments.acp_turn_timeout_ms.is_some() {
        return Err(RuntimeError::validation(
            "validation_conflicting_timeout_fields",
            "provide either --quiescence-timeout-ms or --acp-turn-timeout-ms, not both".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn print_send_help() {
    println!(
        "Usage: agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--quiescence-timeout-ms MS] [--acp-turn-timeout-ms MS] [--request-id ID] [--bundle NAME] [--session NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
