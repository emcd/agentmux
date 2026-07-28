use std::env;

use serde_json::json;

use crate::{
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        error::RuntimeError, paths::RelayRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{NewPeerArguments, shared};

pub(super) fn run_agentmux_new(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_new_help();
        return Ok(());
    }

    let parsed = parse_new_arguments(arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let roots = shared::resolve_roots(&parsed.runtime, &current_directory)?;
    ensure_starter_configuration_layout(&roots)?;
    let resolved_session = resolve_tui_session_identity(
        &roots.configuration_roots,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    let destination = shared::resolve_credential_destination(
        parsed.output_path.as_deref(),
        parsed.write_to_config,
    )?;

    let response = request_relay(
        &relay_paths.relay_socket,
        resolved_session.namespace.as_str(),
        resolved_session.session_id.as_str(),
        &RelayRequest::NewPeer {
            principal_id: parsed.principal_id.clone(),
            scope: parsed.scope.clone(),
            destination,
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;

    match response {
        RelayResponse::NewPeer {
            schema_version,
            principal_id,
            principal_type,
            psk,
            written_path,
            config_snippet,
        } => {
            if parsed.output_json {
                let payload = json!({
                    "schema_version": schema_version,
                    "principal_id": principal_id,
                    "principal_type": principal_type,
                    "psk": psk,
                    "written_path": written_path,
                    "config_snippet": config_snippet,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).map_err(|source| {
                        RuntimeError::io(
                            "encode new peer response json",
                            std::io::Error::other(source),
                        )
                    })?
                );
            } else {
                println!("principal_id={principal_id} principal_type={principal_type}");
                match (psk.as_deref(), written_path.as_deref()) {
                    (Some(value), _) => println!("psk={value}"),
                    (None, Some(path)) => println!("psk written to {path}"),
                    (None, None) => {}
                }
                println!("{config_snippet}");
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

fn parse_new_arguments(arguments: &[String]) -> Result<NewPeerArguments, RuntimeError> {
    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing new subcommand; expected 'peer'".to_string(),
        ));
    };
    if subcommand != "peer" {
        return Err(RuntimeError::InvalidArgument {
            argument: subcommand.to_string(),
            message: "unknown new subcommand".to_string(),
        });
    }

    let mut principal_id: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut write_to_config = false;
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
            "--scope" => scope = Some(shared::take_value(arguments, &mut index, "--scope")?),
            "--output" => {
                output_path = Some(shared::take_value(arguments, &mut index, "--output")?)
            }
            "--write-config" => write_to_config = true,
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
            "new peer requires a <principal_id> argument".to_string(),
        ));
    };
    Ok(NewPeerArguments {
        principal_id,
        scope,
        output_path,
        write_to_config,
        bundle_name,
        session_selector,
        output_json,
        runtime,
    })
}

pub(super) fn print_new_help() {
    println!(
        "Usage: agentmux new peer <principal_id> [--scope SCOPE] [--output PATH | --write-config] [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
