use serde_json::json;

use crate::{
    relay::{
        RelayRequest, RelayResponse,
        peer_scope::{is_relay_principal_id, parse_peer_scope},
        request_relay,
    },
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
            diagnostics,
        } => {
            // Advisories go to stderr in both modes, so a caller parsing stdout
            // as JSON still sees them and a caller reading the human output is
            // not left to notice a warning buried among the credential lines.
            // They never change the exit status: the principal was registered.
            for diagnostic in &diagnostics {
                eprintln!(
                    "agentmux new peer: {}: {}",
                    diagnostic.code, diagnostic.message
                );
            }
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
    // Pre-submission grammar check for peer relay scopes so malformed input
    // fails before any relay contact. A principal aiming at the relay
    // namespace with a malformed identity is rejected here too; other
    // principal types keep their own scope model and pass through for the
    // relay to judge.
    let normalized_principal = principal_id.trim();
    if normalized_principal.ends_with("@RELAY") && !is_relay_principal_id(normalized_principal) {
        return Err(RuntimeError::validation(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form".to_string(),
        ));
    }
    if let Some(scope) = scope.as_deref()
        && is_relay_principal_id(normalized_principal)
        && let Err(error) = parse_peer_scope(Some(scope))
    {
        return Err(RuntimeError::validation(error.code, error.message));
    }
    // The normalized (trimmed) identity is submitted, so validation and the
    // relay observe the same principal.
    Ok(NewPeerArguments {
        principal_id: normalized_principal.to_string(),
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
        "Usage: agentmux new peer <principal_id> [--scope SCOPE] [--output PATH | --write-config] [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]\n\
         \n\
         principal_id is <id>@<namespace>: <id>@RELAY registers a peer relay,\n\
         <id>@GLOBAL a user, <id>@EXTERNAL an application, and <id>@<bundle>\n\
         a bundle-scoped session. Every value needs the @ separator.\n\
         \n\
         --scope names what the credential may observe: for @RELAY see\n\
         SCOPE below; for @EXTERNAL a session@bundle identity, a bare bundle\n\
         name, or omitted. none/self/home/all are session-policy tiers, not\n\
         scopes; using one matches a namespace literally named that and\n\
         raises an advisory without failing the command.\n\
         \n\
         --output PATH writes the PSK to a caller-named absolute path whose\n\
         parent directory already exists; parents are never created and a\n\
         symlinked target is refused. --write-config writes only session\n\
         principals to their relay-owned identity path, creating parents, and\n\
         is rejected for relay/user/application principals. The flags are\n\
         mutually exclusive; with neither, the PSK prints to stdout once.\n\
         \n\
         --bundle NAME and --as-session NAME resolve the calling identity:\n\
         explicit flags first, then the ui.toml default-bundle and the\n\
         users.toml default-session. Outside a session context pass --bundle\n\
         (and usually --as-session); otherwise the command fails with\n\
         validation_unknown_bundle or validation_unknown_session.\n\
         \n\
         --json keeps stdout parseable; advisories go to stderr in both modes\n\
         and never change the exit status."
    );
    println!(
        "SCOPE is '*' for every namespace with addressable principals (including GLOBAL and future addressable namespace types), a comma-separated set of explicit namespaces, or omitted for no rights."
    );
}
