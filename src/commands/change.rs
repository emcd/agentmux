use serde_json::json;

use crate::{
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        error::RuntimeError, paths::RelayRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{ChangePskArguments, ChangeScopeArguments, shared};

pub(super) fn run_agentmux_change(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_change_help();
        return Ok(());
    }

    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing change subcommand; expected 'psk' or 'scope'".to_string(),
        ));
    };
    match subcommand {
        "psk" => run_change_psk(&arguments[1..]),
        "scope" => run_change_scope(&arguments[1..]),
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown change subcommand".to_string(),
        }),
    }
}

fn run_change_psk(arguments: &[String]) -> Result<(), RuntimeError> {
    let parsed = parse_change_psk_arguments(arguments)?;
    let roots = shared::resolve_roots(&parsed.runtime)?;
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
        &RelayRequest::ChangePsk {
            principal_id: parsed.principal_id.clone(),
            destination,
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;

    match response {
        RelayResponse::ChangePsk {
            schema_version,
            principal_id,
            psk,
            written_path,
        } => {
            if parsed.output_json {
                let payload = json!({
                    "schema_version": schema_version,
                    "principal_id": principal_id,
                    "psk": psk,
                    "written_path": written_path,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).map_err(|source| {
                        RuntimeError::io(
                            "encode change psk response json",
                            std::io::Error::other(source),
                        )
                    })?
                );
            } else {
                println!("principal_id={principal_id}");
                match (psk.as_deref(), written_path.as_deref()) {
                    (Some(value), _) => println!("psk={value}"),
                    (None, Some(path)) => println!("psk written to {path}"),
                    (None, None) => {}
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

fn run_change_scope(arguments: &[String]) -> Result<(), RuntimeError> {
    let parsed = parse_change_scope_arguments(arguments)?;
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
        &RelayRequest::ChangeScope {
            principal_id: parsed.principal_id.clone(),
            scope: parsed.scope.clone(),
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;

    match response {
        RelayResponse::ChangeScope {
            schema_version,
            principal_id,
            scope,
            diagnostics,
        } => {
            for diagnostic in &diagnostics {
                eprintln!(
                    "agentmux change scope: {}: {}",
                    diagnostic.code, diagnostic.message
                );
            }
            if parsed.output_json {
                let payload = json!({
                    "schema_version": schema_version,
                    "principal_id": principal_id,
                    "scope": scope,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).map_err(|source| {
                        RuntimeError::io(
                            "encode change scope response json",
                            std::io::Error::other(source),
                        )
                    })?
                );
            } else {
                println!("principal_id={principal_id} scope={scope}");
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

fn parse_change_psk_arguments(arguments: &[String]) -> Result<ChangePskArguments, RuntimeError> {
    let mut principal_id: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut write_to_config = false;
    let mut bundle_name: Option<String> = None;
    let mut session_selector: Option<String> = None;
    let mut output_json = false;
    let mut runtime = super::RuntimeArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
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
            "change psk requires a <principal_id> argument".to_string(),
        ));
    };
    Ok(ChangePskArguments {
        principal_id,
        output_path,
        write_to_config,
        bundle_name,
        session_selector,
        output_json,
        runtime,
    })
}

fn parse_change_scope_arguments(
    arguments: &[String],
) -> Result<ChangeScopeArguments, RuntimeError> {
    let mut principal_id: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut bundle_name: Option<String> = None;
    let mut session_selector: Option<String> = None;
    let mut output_json = false;
    let mut runtime = super::RuntimeArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--scope" => scope = Some(shared::take_value(arguments, &mut index, "--scope")?),
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
            "change scope requires a <principal_id> argument".to_string(),
        ));
    };
    let Some(scope) = scope else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "change scope requires --scope SCOPE (pass --scope '' to clear the grant)".to_string(),
        ));
    };
    Ok(ChangeScopeArguments {
        principal_id,
        scope,
        bundle_name,
        session_selector,
        output_json,
        runtime,
    })
}

pub(super) fn print_change_help() {
    println!(
        "Usage: agentmux change psk <principal_id> [--output PATH | --write-config] [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
    println!(
        "Usage: agentmux change scope <principal_id> --scope SCOPE [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
    println!(
        "SCOPE is '*' for every namespace with addressable principals (including GLOBAL and future addressable namespace types), a comma-separated set of explicit namespaces, or '' to clear the grant."
    );
}
