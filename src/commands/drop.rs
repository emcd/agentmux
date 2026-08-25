use serde_json::{Map, Value, json};

use crate::{
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        error::RuntimeError, paths::RelayRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{DropPeerArguments, shared};

pub(super) fn run_agentmux_drop(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_drop_help();
        return Ok(());
    }

    let parsed = parse_drop_arguments(arguments)?;
    let roots = shared::resolve_roots(&parsed.runtime)?;
    ensure_starter_configuration_layout(&roots)?;
    let resolved_session = resolve_tui_session_identity(
        &roots.configuration_roots,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);

    let response = request_relay(
        &relay_paths.relay_socket,
        resolved_session.namespace.as_str(),
        resolved_session.session_id.as_str(),
        &RelayRequest::DropPeer {
            principal_id: parsed.principal_id.clone(),
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;

    match response {
        RelayResponse::DropPeer {
            schema_version,
            principal_id,
            principal_type,
            credential_path,
        } => {
            if parsed.output_json {
                // Built conditionally so `credential_path` is absent rather than
                // null for a principal the relay owns no credential location
                // for, matching what the relay and the MCP tool emit.
                let mut payload = Map::new();
                payload.insert("schema_version".to_string(), json!(schema_version));
                payload.insert("principal_id".to_string(), json!(principal_id));
                payload.insert("principal_type".to_string(), json!(principal_type));
                if let Some(path) = credential_path.as_deref() {
                    payload.insert("credential_path".to_string(), json!(path));
                }
                let payload = Value::Object(payload);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).map_err(|source| {
                        RuntimeError::io(
                            "encode drop peer response json",
                            std::io::Error::other(source),
                        )
                    })?
                );
            } else {
                println!("principal_id={principal_id} principal_type={principal_type}");
                // Reported rather than deleted: once the record is gone the file
                // authenticates nothing, and the relay cannot know where the
                // operator distributed it.
                if let Some(path) = credential_path.as_deref() {
                    println!("credential file left in place at {path}");
                }
            }
            Ok(())
        }
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        other => Err(RuntimeError::validation(
            "internal_unexpected_failure",
            format!("relay returned unexpected response variant: {other:?}"),
        )),
    }
}

fn parse_drop_arguments(arguments: &[String]) -> Result<DropPeerArguments, RuntimeError> {
    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing drop subcommand; expected 'peer'".to_string(),
        ));
    };
    if subcommand != "peer" {
        return Err(RuntimeError::InvalidArgument {
            argument: subcommand.to_string(),
            message: "unknown drop subcommand".to_string(),
        });
    }

    let mut principal_id: Option<String> = None;
    let mut bundle_name: Option<String> = None;
    let mut session_selector: Option<String> = None;
    let mut output_json = false;
    let mut runtime = super::RuntimeArguments::default();
    let mut index = 1usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?)
            }
            "--as-session" => {
                session_selector = Some(shared::take_value(arguments, &mut index, "--as-session")?)
            }
            "--json" => output_json = true,
            value if value.starts_with('-') => {
                return Err(RuntimeError::InvalidArgument {
                    argument: value.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
            value => {
                if principal_id.is_some() {
                    return Err(RuntimeError::InvalidArgument {
                        argument: value.to_string(),
                        message: "unexpected positional argument".to_string(),
                    });
                }
                principal_id = Some(value.to_string());
            }
        }
        index += 1;
    }

    let Some(principal_id) = principal_id else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "drop peer requires a <principal_id> argument".to_string(),
        ));
    };
    Ok(DropPeerArguments {
        principal_id,
        bundle_name,
        session_selector,
        output_json,
        runtime,
    })
}

pub(super) fn print_drop_help() {
    println!(
        "Usage: agentmux drop peer <principal_id> [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
}
