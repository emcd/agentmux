//! Shared command execution for agentmux binaries.

use std::{
    env,
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::{
    configuration::{ConfigurationError, load_bundle_configuration},
    mcp::McpConfiguration,
    relay::{
        ChatDeliveryMode, RelayError, RelayRequest, RelayResponse, reconcile_bundle, request_relay,
    },
    runtime::{
        association::{
            McpAssociationCli, WorkspaceContext, load_local_mcp_overrides, resolve_association,
            resolve_sender_session,
        },
        bootstrap::{acquire_relay_runtime_lock, bind_relay_listener},
        error::RuntimeError,
        inscriptions::{
            configure_process_inscriptions, emit_inscription, mcp_inscriptions_path,
            relay_inscriptions_path,
        },
        paths::{
            BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots, ensure_bundle_runtime_directory,
        },
        starter::ensure_starter_configuration_layout,
    },
};

#[derive(Clone, Debug, Default)]
struct RuntimeArguments {
    configuration_root: Option<PathBuf>,
    state_root: Option<PathBuf>,
    inscriptions_root: Option<PathBuf>,
    repository_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct RelayHostArguments {
    bundle_name: String,
    runtime: RuntimeArguments,
}

#[derive(Clone, Debug, Default)]
struct McpHostArguments {
    bundle_name: Option<String>,
    session_name: Option<String>,
    runtime: RuntimeArguments,
}

#[derive(Clone, Debug, Default)]
struct ListArguments {
    bundle_name: Option<String>,
    sender_session: Option<String>,
    output_json: bool,
    runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
struct SendArguments {
    bundle_name: Option<String>,
    sender_session: Option<String>,
    request_id: Option<String>,
    message: String,
    targets: Vec<String>,
    broadcast: bool,
    delivery_mode: ChatDeliveryMode,
    quiescence_timeout_ms: Option<u64>,
    output_json: bool,
    runtime: RuntimeArguments,
}

/// Runs the unified `agentmux` CLI entrypoint.
pub async fn run_agentmux(arguments: Vec<String>) -> Result<(), RuntimeError> {
    if arguments.is_empty() {
        print_agentmux_help();
        return Ok(());
    }

    match arguments[0].as_str() {
        "--help" | "-h" => {
            print_agentmux_help();
            Ok(())
        }
        "host" => run_agentmux_host(&arguments[1..]).await,
        "list" => run_agentmux_list(&arguments[1..]),
        "send" => run_agentmux_send(&arguments[1..]),
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown subcommand".to_string(),
        }),
    }
}

/// Runs compatibility `agentmux-relay` behavior.
pub fn run_agentmux_relay_legacy(arguments: Vec<String>) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_legacy_relay_help();
        return Ok(());
    }
    let parsed = parse_relay_legacy_arguments(arguments)?;
    run_relay_host(parsed)
}

/// Runs compatibility `agentmux-mcp` behavior.
pub async fn run_agentmux_mcp_legacy(arguments: Vec<String>) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_legacy_mcp_help();
        return Ok(());
    }
    let parsed = parse_mcp_legacy_arguments(arguments)?;
    run_mcp_host(parsed).await
}

async fn run_agentmux_host(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments.is_empty() {
        return Err(RuntimeError::InvalidArgument {
            argument: "host".to_string(),
            message: "missing mode; expected relay or mcp".to_string(),
        });
    }
    if arguments[0] == "--help" || arguments[0] == "-h" {
        print_host_help();
        return Ok(());
    }

    match arguments[0].as_str() {
        "relay" => {
            if arguments[1..]
                .iter()
                .any(|value| value == "--help" || value == "-h")
            {
                print_host_relay_help();
                return Ok(());
            }
            run_relay_host(parse_host_relay_arguments(&arguments[1..])?)
        }
        "mcp" => {
            if arguments[1..]
                .iter()
                .any(|value| value == "--help" || value == "-h")
            {
                print_host_mcp_help();
                return Ok(());
            }
            run_mcp_host(parse_host_mcp_arguments(&arguments[1..])?).await
        }
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown host mode; expected relay or mcp".to_string(),
        }),
    }
}

fn run_agentmux_list(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_list_help();
        return Ok(());
    }
    let parsed = parse_list_arguments(arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: parsed.bundle_name.clone(),
            session_name: parsed.sender_session.clone(),
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let roots = resolve_roots(&parsed.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(map_bundle_load_error)?;
    let sender_session = if parsed.sender_session.is_some() {
        Some(resolve_sender_session(
            &bundle,
            &association.session_name,
            &current_directory,
        )?)
    } else {
        resolve_sender_session(&bundle, &association.session_name, &current_directory).ok()
    };
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    let response = request_relay(&paths.relay_socket, &RelayRequest::List { sender_session })
        .map_err(|source| map_relay_request_failure(&paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::List {
            schema_version,
            bundle_name,
            recipients,
        } => json!({
            "schema_version": schema_version,
            "bundle_name": bundle_name,
            "recipients": recipients,
        }),
        RelayResponse::Error { error } => return Err(map_relay_error(error)),
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
                RuntimeError::io("encode list response json", std::io::Error::other(source))
            })?
        );
    } else {
        println!(
            "bundle={}",
            payload["bundle_name"].as_str().unwrap_or_default()
        );
        if let Some(recipients) = payload["recipients"].as_array() {
            for recipient in recipients {
                let session = recipient["session_name"].as_str().unwrap_or_default();
                if let Some(display_name) = recipient["display_name"].as_str() {
                    println!("{session}\t{display_name}");
                } else {
                    println!("{session}");
                }
            }
        }
    }
    Ok(())
}

fn run_agentmux_send(arguments: &[String]) -> Result<(), RuntimeError> {
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
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: parsed.bundle_name.clone(),
            session_name: parsed.sender_session.clone(),
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let roots = resolve_roots(&parsed.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(map_bundle_load_error)?;
    let sender_session =
        resolve_sender_session(&bundle, &association.session_name, &current_directory)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    let response = request_relay(
        &paths.relay_socket,
        &RelayRequest::Chat {
            request_id: parsed.request_id.clone(),
            sender_session,
            message: parsed.message.clone(),
            targets: parsed.targets.clone(),
            broadcast: parsed.broadcast,
            delivery_mode: parsed.delivery_mode,
            quiet_window_ms: None,
            quiescence_timeout_ms: parsed.quiescence_timeout_ms,
        },
    )
    .map_err(|source| map_relay_request_failure(&paths.relay_socket, source))?;
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
        RelayResponse::Error { error } => return Err(map_relay_error(error)),
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

fn run_relay_host(arguments: RelayHostArguments) -> Result<(), RuntimeError> {
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let overrides = RuntimeRootOverrides {
        configuration_root: arguments.runtime.configuration_root,
        state_root: arguments.runtime.state_root,
        inscriptions_root: arguments.runtime.inscriptions_root,
        repository_root: arguments
            .runtime
            .repository_root
            .or(Some(current_directory)),
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &arguments.bundle_name)?;
    configure_process_inscriptions(&relay_inscriptions_path(
        &roots.inscriptions_root,
        &paths.bundle_name,
    ))?;
    emit_inscription(
        "relay.startup",
        &json!({
            "bundle_name": paths.bundle_name,
            "relay_socket": paths.relay_socket,
            "tmux_socket": paths.tmux_socket,
            "configuration_root": roots.configuration_root,
            "state_root": roots.state_root,
            "inscriptions_root": roots.inscriptions_root,
        }),
    );
    ensure_bundle_runtime_directory(&paths)?;
    let _runtime_lock = acquire_relay_runtime_lock(&paths)?;
    let report = reconcile_bundle(
        &roots.configuration_root,
        &paths.bundle_name,
        &paths.tmux_socket,
    )
    .map_err(map_reconcile_error)?;
    let listener = bind_relay_listener(&paths)?;
    println!(
        "agentmux-relay listening bundle={} socket={} bootstrap={:?} created={} pruned={}",
        paths.bundle_name,
        paths.relay_socket.display(),
        report.bootstrap_session,
        report.created_sessions.len(),
        report.pruned_sessions.len(),
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(source) =
                    crate::relay::serve_connection(&mut stream, &roots.configuration_root, &paths)
                {
                    emit_inscription(
                        "relay.request_failed",
                        &json!({"error": source.to_string()}),
                    );
                    eprintln!("agentmux-relay: request handling failed: {source}");
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(RuntimeError::io("accept relay socket connection", source));
            }
        }
    }
    Ok(())
}

async fn run_mcp_host(arguments: McpHostArguments) -> Result<(), RuntimeError> {
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: arguments.bundle_name.clone(),
            session_name: arguments.session_name.clone(),
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let roots = resolve_roots(&arguments.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(map_bundle_load_error)?;
    let session_name =
        resolve_sender_session(&bundle, &association.session_name, &current_directory)?;
    configure_process_inscriptions(&mcp_inscriptions_path(
        &roots.inscriptions_root,
        &association.bundle_name,
        &session_name,
    ))?;
    emit_inscription(
        "mcp.startup",
        &json!({
            "bundle_name": association.bundle_name,
            "session_name": session_name,
            "configuration_root": roots.configuration_root,
            "state_root": roots.state_root,
            "inscriptions_root": roots.inscriptions_root,
        }),
    );
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    crate::mcp::run(McpConfiguration {
        bundle_paths: paths,
        sender_session: Some(session_name),
    })
    .await
    .map_err(|source| RuntimeError::io("run MCP stdio service", std::io::Error::other(source)))
}

fn parse_relay_legacy_arguments(
    arguments: Vec<String>,
) -> Result<RelayHostArguments, RuntimeError> {
    let mut parsed = RelayHostArguments {
        bundle_name: "default".to_string(),
        runtime: RuntimeArguments::default(),
    };
    let mut index = 0usize;
    while index < arguments.len() {
        if parse_runtime_flag(&arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" => {
                parsed.bundle_name = take_value(&arguments, &mut index, "--bundle")?;
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
    Ok(parsed)
}

fn parse_mcp_legacy_arguments(arguments: Vec<String>) -> Result<McpHostArguments, RuntimeError> {
    let mut parsed = McpHostArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if parse_runtime_flag(&arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle-name" | "--bundle" => {
                parsed.bundle_name = Some(take_value(&arguments, &mut index, "--bundle-name")?);
            }
            "--session-name" => {
                parsed.session_name = Some(take_value(&arguments, &mut index, "--session-name")?);
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
    Ok(parsed)
}

fn parse_host_relay_arguments(arguments: &[String]) -> Result<RelayHostArguments, RuntimeError> {
    if arguments.is_empty() {
        return Err(RuntimeError::InvalidArgument {
            argument: "<bundle-id>".to_string(),
            message: "missing value".to_string(),
        });
    }
    if arguments[0].starts_with('-') {
        return Err(RuntimeError::InvalidArgument {
            argument: "<bundle-id>".to_string(),
            message: "missing value".to_string(),
        });
    }

    let mut parsed = RelayHostArguments {
        bundle_name: arguments[0].clone(),
        runtime: RuntimeArguments::default(),
    };
    let mut index = 1usize;
    while index < arguments.len() {
        if parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        return Err(RuntimeError::InvalidArgument {
            argument: arguments[index].clone(),
            message: "unknown argument".to_string(),
        });
    }
    Ok(parsed)
}

fn parse_host_mcp_arguments(arguments: &[String]) -> Result<McpHostArguments, RuntimeError> {
    let mut parsed = McpHostArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(take_value(arguments, &mut index, "--bundle")?);
            }
            "--session-name" => {
                parsed.session_name = Some(take_value(arguments, &mut index, "--session-name")?);
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
    Ok(parsed)
}

fn parse_list_arguments(arguments: &[String]) -> Result<ListArguments, RuntimeError> {
    let mut parsed = ListArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(take_value(arguments, &mut index, "--bundle")?);
            }
            "--sender" | "--session-name" => {
                parsed.sender_session = Some(take_value(arguments, &mut index, "--sender")?);
            }
            "--json" => parsed.output_json = true,
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_send_arguments(arguments: &[String]) -> Result<SendArguments, RuntimeError> {
    let mut bundle_name = None;
    let mut sender_session = None;
    let mut request_id = None;
    let mut targets = Vec::<String>::new();
    let mut broadcast = false;
    let mut message = None;
    let mut delivery_mode = ChatDeliveryMode::Async;
    let mut quiescence_timeout_ms = None;
    let mut output_json = false;
    let mut runtime = RuntimeArguments::default();
    let mut index = 0usize;

    while index < arguments.len() {
        if parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                bundle_name = Some(take_value(arguments, &mut index, "--bundle")?);
            }
            "--sender" | "--session-name" => {
                sender_session = Some(take_value(arguments, &mut index, "--sender")?);
            }
            "--request-id" => request_id = Some(take_value(arguments, &mut index, "--request-id")?),
            "--target" => targets.push(take_value(arguments, &mut index, "--target")?),
            "--broadcast" => broadcast = true,
            "--message" => message = Some(take_value(arguments, &mut index, "--message")?),
            "--delivery-mode" => {
                let value = take_value(arguments, &mut index, "--delivery-mode")?;
                delivery_mode = parse_delivery_mode(value.as_str())?;
            }
            "--quiescence-timeout-ms" => {
                let value = take_value(arguments, &mut index, "--quiescence-timeout-ms")?;
                quiescence_timeout_ms = Some(parse_positive_u64(
                    value.as_str(),
                    "--quiescence-timeout-ms",
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
        sender_session,
        request_id,
        message,
        targets,
        broadcast,
        delivery_mode,
        quiescence_timeout_ms,
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
    Ok(())
}

fn parse_runtime_flag(
    arguments: &[String],
    index: &mut usize,
    runtime: &mut RuntimeArguments,
) -> Result<bool, RuntimeError> {
    match arguments[*index].as_str() {
        "--config-directory" => {
            runtime.configuration_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--config-directory",
            )?));
            Ok(true)
        }
        "--state-directory" => {
            runtime.state_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--state-directory",
            )?));
            Ok(true)
        }
        "--inscriptions-directory" | "--logs-directory" => {
            runtime.inscriptions_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--inscriptions-directory",
            )?));
            Ok(true)
        }
        "--repository-root" => {
            runtime.repository_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--repository-root",
            )?));
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn resolve_roots(
    runtime: &RuntimeArguments,
    workspace: &WorkspaceContext,
    local_overrides: Option<&crate::runtime::association::McpAssociationOverrides>,
) -> Result<RuntimeRoots, RuntimeError> {
    let configuration_root = runtime
        .configuration_root
        .clone()
        .or_else(|| local_overrides.and_then(|overrides| overrides.config_root.clone()));
    RuntimeRoots::resolve(&RuntimeRootOverrides {
        configuration_root,
        state_root: runtime.state_root.clone(),
        inscriptions_root: runtime.inscriptions_root.clone(),
        repository_root: runtime
            .repository_root
            .clone()
            .or_else(|| workspace.debug_repository_root()),
    })
}

fn parse_delivery_mode(value: &str) -> Result<ChatDeliveryMode, RuntimeError> {
    match value {
        "async" => Ok(ChatDeliveryMode::Async),
        "sync" => Ok(ChatDeliveryMode::Sync),
        _ => Err(RuntimeError::validation(
            "validation_invalid_delivery_mode",
            format!("unsupported delivery mode '{value}'; expected async or sync"),
        )),
    }
}

fn parse_positive_u64(value: &str, flag: &str) -> Result<u64, RuntimeError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| RuntimeError::InvalidArgument {
            argument: flag.to_string(),
            message: format!("invalid numeric value '{value}'"),
        })?;
    if parsed == 0 {
        return Err(RuntimeError::validation(
            "validation_invalid_quiescence_timeout",
            "quiescence timeout override must be greater than zero milliseconds".to_string(),
        ));
    }
    Ok(parsed)
}

fn take_value(arguments: &[String], index: &mut usize, flag: &str) -> Result<String, RuntimeError> {
    *index += 1;
    let Some(value) = arguments.get(*index) else {
        return Err(RuntimeError::InvalidArgument {
            argument: flag.to_string(),
            message: "missing value".to_string(),
        });
    };
    Ok(value.to_string())
}

fn map_reconcile_error(source: RelayError) -> RuntimeError {
    if source.code.starts_with("validation_") {
        return RuntimeError::validation(source.code, source.message);
    }
    let message = source.message.clone();
    RuntimeError::io(message, std::io::Error::other(format!("{source:?}")))
}

fn map_bundle_load_error(source: ConfigurationError) -> RuntimeError {
    match source {
        ConfigurationError::UnknownBundle { bundle_name, .. } => RuntimeError::validation(
            "validation_unknown_bundle",
            format!("bundle '{}' is not configured", bundle_name),
        ),
        ConfigurationError::AmbiguousSender { .. } => RuntimeError::validation(
            "validation_unknown_sender",
            "sender association is ambiguous".to_string(),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!(
                "invalid bundle configuration {}: {}",
                path.display(),
                message
            ),
        ),
        ConfigurationError::Io { context, source } => RuntimeError::io(context, source),
    }
}

fn map_relay_error(error: RelayError) -> RuntimeError {
    if error.code.starts_with("validation_") {
        return RuntimeError::validation(error.code, error.message);
    }
    RuntimeError::io(
        error.message,
        std::io::Error::other("relay returned internal error"),
    )
}

fn map_relay_request_failure(socket_path: &Path, source: std::io::Error) -> RuntimeError {
    if is_relay_unavailable_error(&source) {
        return RuntimeError::validation(
            "relay_unavailable",
            format!(
                "relay is unavailable at {}; start host relay with matching bundle and state-directory",
                socket_path.display()
            ),
        );
    }
    RuntimeError::io(
        format!("relay request failed for {}", socket_path.display()),
        source,
    )
}

fn is_relay_unavailable_error(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}

fn print_agentmux_help() {
    println!(
        "Usage: agentmux <command> [options]\n\nCommands:\n  host relay <bundle-id> [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  list [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--quiescence-timeout-ms MS] [--request-id ID] [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_host_help() {
    println!("Usage: agentmux host <relay|mcp> [options]");
}

fn print_host_relay_help() {
    println!(
        "Usage: agentmux host relay <bundle-id> [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_host_mcp_help() {
    println!(
        "Usage: agentmux host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_list_help() {
    println!(
        "Usage: agentmux list [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_send_help() {
    println!(
        "Usage: agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--quiescence-timeout-ms MS] [--request-id ID] [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_legacy_relay_help() {
    println!(
        "Usage: agentmux-relay [--bundle NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_legacy_mcp_help() {
    println!(
        "Usage: agentmux-mcp [--bundle-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH] [--session-name NAME]"
    );
}
