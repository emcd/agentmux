//! Shared command execution for agentmux binaries.

use std::{
    env, fs,
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde_json::{Map, Value, json};

use crate::{
    configuration::{
        ConfigurationError, RESERVED_GROUP_ALL, load_bundle_configuration,
        load_bundle_group_memberships,
    },
    mcp::McpConfiguration,
    relay::{
        ChatDeliveryMode, RelayError, RelayRequest, RelayResponse, reconcile_bundle, request_relay,
        shutdown_bundle_runtime, wait_for_async_delivery_shutdown,
    },
    runtime::{
        association::{
            McpAssociationCli, WorkspaceContext, load_local_mcp_overrides, resolve_association,
            resolve_sender_session,
        },
        bootstrap::{
            BootstrapOptions, acquire_relay_runtime_lock, bind_relay_listener, bootstrap_relay,
            relay_runtime_lock_is_held, resolve_relay_program, spawn_relay_process,
        },
        error::RuntimeError,
        inscriptions::{
            configure_process_inscriptions, emit_inscription, mcp_inscriptions_path,
            relay_inscriptions_path,
        },
        paths::{
            BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots, ensure_bundle_runtime_directory,
        },
        signals::{install_shutdown_signal_handlers, shutdown_requested},
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
    selector: RelayHostSelector,
    runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
enum RelayHostSelector {
    Bundle(String),
    Group(String),
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

#[derive(Clone, Debug)]
struct RelayHostStartupBundle {
    bundle_name: String,
    outcome: String,
    reason_code: Option<String>,
    reason: Option<String>,
}

#[derive(Clone, Debug)]
struct RelayHostStartupSummary {
    schema_version: u32,
    host_mode: String,
    group_name: Option<String>,
    bundles: Vec<RelayHostStartupBundle>,
    hosted_bundle_count: usize,
    skipped_bundle_count: usize,
    failed_bundle_count: usize,
    hosted_any: bool,
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

    match arguments.selector {
        RelayHostSelector::Bundle(bundle_name) => {
            run_relay_host_single(&roots, bundle_name.as_str())
        }
        RelayHostSelector::Group(group_name) => run_relay_host_group(&roots, group_name.as_str()),
    }
}

fn run_relay_host_single(roots: &RuntimeRoots, bundle_name: &str) -> Result<(), RuntimeError> {
    let paths = BundleRuntimePaths::resolve(&roots.state_root, bundle_name)?;
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
    listener
        .set_nonblocking(true)
        .map_err(|source| RuntimeError::io("set relay socket listener nonblocking", source))?;
    let _signal_handlers = install_shutdown_signal_handlers()?;
    println!(
        "agentmux host relay listening bundle={} socket={} bootstrap={:?} created={} pruned={}",
        paths.bundle_name,
        paths.relay_socket.display(),
        report.bootstrap_session,
        report.created_sessions.len(),
        report.pruned_sessions.len(),
    );
    let summary = build_startup_summary(
        "single_bundle",
        None,
        vec![hosted_startup_bundle(paths.bundle_name.as_str())],
    );
    emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
    render_startup_summary(&summary);
    let mut accept_error = None::<RuntimeError>;
    while !shutdown_requested() {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Err(source) =
                    crate::relay::serve_connection(&mut stream, &roots.configuration_root, &paths)
                {
                    emit_inscription(
                        "relay.request_failed",
                        &json!({"error": source.to_string()}),
                    );
                    eprintln!("agentmux host relay: request handling failed: {source}");
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => {
                accept_error = Some(RuntimeError::io("accept relay socket connection", source));
                break;
            }
        }
    }
    if shutdown_requested() {
        emit_inscription("relay.shutdown.signal", &json!({"signal": "termination"}));
    }
    let async_workers_remaining = if shutdown_requested() {
        wait_for_async_delivery_shutdown(Duration::from_millis(1_500))
    } else {
        0
    };
    drop(listener);
    remove_relay_socket_file(&paths.relay_socket)?;
    let shutdown = shutdown_bundle_runtime(&paths.tmux_socket).map_err(map_reconcile_error)?;
    emit_inscription(
        "relay.shutdown.complete",
        &json!({
            "bundle_name": paths.bundle_name,
            "pruned_count": shutdown.pruned_sessions.len(),
            "killed_tmux_server": shutdown.killed_tmux_server,
            "pruned_sessions": shutdown.pruned_sessions,
            "async_workers_remaining": async_workers_remaining,
        }),
    );
    if let Some(error) = accept_error {
        return Err(error);
    }
    Ok(())
}

fn run_relay_host_group(roots: &RuntimeRoots, group_name: &str) -> Result<(), RuntimeError> {
    validate_group_selector_name(group_name)?;
    let memberships =
        load_bundle_group_memberships(&roots.configuration_root).map_err(map_bundle_load_error)?;
    let selected_bundles = resolve_group_bundles(memberships, group_name)?;
    let relay_program = resolve_relay_program()?;

    let mut outcomes = Vec::with_capacity(selected_bundles.len());
    for bundle_name in selected_bundles {
        outcomes.push(host_group_bundle(
            roots,
            relay_program.as_path(),
            bundle_name.as_str(),
        ));
    }

    let summary = build_startup_summary("bundle_group", Some(group_name.to_string()), outcomes);
    render_startup_summary(&summary);
    if !summary.hosted_any {
        return Err(RuntimeError::validation(
            "validation_no_hosted_bundles",
            format!("no bundles were hosted for group '{group_name}'"),
        ));
    }
    Ok(())
}

fn host_group_bundle(
    roots: &RuntimeRoots,
    relay_program: &Path,
    bundle_name: &str,
) -> RelayHostStartupBundle {
    let paths = match BundleRuntimePaths::resolve(&roots.state_root, bundle_name) {
        Ok(paths) => paths,
        Err(source) => return failed_startup_bundle(bundle_name, source),
    };

    match relay_runtime_lock_is_held(&paths) {
        Ok(true) => {
            return skipped_startup_bundle(
                bundle_name,
                "lock_held",
                "relay runtime lock is already held".to_string(),
            );
        }
        Err(source) => return failed_startup_bundle(bundle_name, source),
        Ok(false) => {}
    }

    let startup = bootstrap_relay(&paths, BootstrapOptions::default(), || {
        let _child = spawn_relay_process(relay_program, &paths, &roots.configuration_root)?;
        Ok(())
    });
    match startup {
        Ok(report) => {
            if report.spawned_relay {
                hosted_startup_bundle(bundle_name)
            } else {
                skipped_startup_bundle(
                    bundle_name,
                    "lock_held",
                    "relay runtime lock is already held".to_string(),
                )
            }
        }
        Err(source) => match relay_runtime_lock_is_held(&paths) {
            Ok(true) => skipped_startup_bundle(
                bundle_name,
                "lock_held",
                "relay runtime lock is already held".to_string(),
            ),
            _ => failed_startup_bundle(bundle_name, source),
        },
    }
}

fn build_startup_summary(
    host_mode: &str,
    group_name: Option<String>,
    bundles: Vec<RelayHostStartupBundle>,
) -> RelayHostStartupSummary {
    let hosted_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "hosted")
        .count();
    let skipped_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "skipped")
        .count();
    let failed_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "failed")
        .count();
    RelayHostStartupSummary {
        schema_version: 1,
        host_mode: host_mode.to_string(),
        group_name,
        bundles,
        hosted_bundle_count,
        skipped_bundle_count,
        failed_bundle_count,
        hosted_any: hosted_bundle_count > 0,
    }
}

fn startup_summary_payload(summary: &RelayHostStartupSummary) -> Value {
    let mut payload = Map::<String, Value>::new();
    payload.insert("schema_version".to_string(), json!(summary.schema_version));
    payload.insert("host_mode".to_string(), json!(summary.host_mode));
    if let Some(group_name) = summary.group_name.as_ref() {
        payload.insert("group_name".to_string(), json!(group_name));
    }
    payload.insert(
        "bundles".to_string(),
        Value::Array(
            summary
                .bundles
                .iter()
                .map(|bundle| {
                    json!({
                        "bundle_name": bundle.bundle_name,
                        "outcome": bundle.outcome,
                        "reason_code": bundle.reason_code,
                        "reason": bundle.reason,
                    })
                })
                .collect::<Vec<_>>(),
        ),
    );
    payload.insert(
        "hosted_bundle_count".to_string(),
        json!(summary.hosted_bundle_count),
    );
    payload.insert(
        "skipped_bundle_count".to_string(),
        json!(summary.skipped_bundle_count),
    );
    payload.insert(
        "failed_bundle_count".to_string(),
        json!(summary.failed_bundle_count),
    );
    payload.insert("hosted_any".to_string(), json!(summary.hosted_any));
    Value::Object(payload)
}

fn render_startup_summary(summary: &RelayHostStartupSummary) {
    match serde_json::to_string(&startup_summary_payload(summary)) {
        Ok(encoded) => println!("{encoded}"),
        Err(source) => {
            eprintln!("agentmux host relay: failed to encode startup summary json: {source}");
        }
    }
    let group_value = summary.group_name.as_deref().unwrap_or("-");
    println!(
        "agentmux host relay summary mode={} group={} hosted={} skipped={} failed={} hosted_any={}",
        summary.host_mode,
        group_value,
        summary.hosted_bundle_count,
        summary.skipped_bundle_count,
        summary.failed_bundle_count,
        summary.hosted_any,
    );
    for bundle in &summary.bundles {
        match (bundle.reason_code.as_deref(), bundle.reason.as_deref()) {
            (Some(reason_code), Some(reason)) => {
                println!(
                    "bundle={} outcome={} reason_code={} reason={}",
                    bundle.bundle_name, bundle.outcome, reason_code, reason
                );
            }
            (Some(reason_code), None) => {
                println!(
                    "bundle={} outcome={} reason_code={}",
                    bundle.bundle_name, bundle.outcome, reason_code
                );
            }
            _ => println!("bundle={} outcome={}", bundle.bundle_name, bundle.outcome),
        }
    }
}

fn hosted_startup_bundle(bundle_name: &str) -> RelayHostStartupBundle {
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "hosted".to_string(),
        reason_code: None,
        reason: None,
    }
}

fn skipped_startup_bundle(
    bundle_name: &str,
    reason_code: &str,
    reason: String,
) -> RelayHostStartupBundle {
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "skipped".to_string(),
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason),
    }
}

fn failed_startup_bundle(bundle_name: &str, source: RuntimeError) -> RelayHostStartupBundle {
    let (reason_code, reason) = runtime_error_reason(&source);
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some(reason_code),
        reason: Some(reason),
    }
}

fn runtime_error_reason(source: &RuntimeError) -> (String, String) {
    match source {
        RuntimeError::Validation { code, message } => (code.clone(), message.clone()),
        RuntimeError::InvalidArgument { message, .. } => {
            ("validation_invalid_arguments".to_string(), message.clone())
        }
        _ => ("runtime_startup_failed".to_string(), source.to_string()),
    }
}

fn resolve_group_bundles(
    memberships: Vec<crate::configuration::BundleGroupMembership>,
    group_name: &str,
) -> Result<Vec<String>, RuntimeError> {
    if group_name == RESERVED_GROUP_ALL {
        return Ok(memberships
            .into_iter()
            .map(|membership| membership.bundle_name)
            .collect::<Vec<_>>());
    }
    let selected = memberships
        .into_iter()
        .filter(|membership| membership.groups.iter().any(|group| group == group_name))
        .map(|membership| membership.bundle_name)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_group",
            format!("group '{}' is not configured", group_name),
        ));
    }
    Ok(selected)
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

fn parse_host_relay_arguments(arguments: &[String]) -> Result<RelayHostArguments, RuntimeError> {
    let mut parsed = RelayHostArguments {
        selector: RelayHostSelector::Bundle(String::new()),
        runtime: RuntimeArguments::default(),
    };
    let mut positional_bundle = None::<String>;
    let mut group_name = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        if parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--group" => group_name = Some(take_value(arguments, &mut index, "--group")?),
            "--all" | "--include-bundle" | "--exclude-bundle" => {
                return Err(RuntimeError::validation(
                    "validation_invalid_arguments",
                    format!("'{}' is not supported in relay host MVP", arguments[index]),
                ));
            }
            value if !value.starts_with('-') => {
                if positional_bundle.is_some() {
                    return Err(RuntimeError::InvalidArgument {
                        argument: value.to_string(),
                        message: "unknown argument".to_string(),
                    });
                }
                positional_bundle = Some(value.to_string());
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

    parsed.selector = match (positional_bundle, group_name) {
        (Some(_), Some(_)) => {
            return Err(RuntimeError::validation(
                "validation_conflicting_selectors",
                "provide either positional <bundle-id> or --group <GROUP>, not both".to_string(),
            ));
        }
        (None, None) => {
            return Err(RuntimeError::InvalidArgument {
                argument: "<bundle-id>|--group".to_string(),
                message: "missing selector".to_string(),
            });
        }
        (Some(bundle_name), None) => RelayHostSelector::Bundle(bundle_name),
        (None, Some(group_name)) => {
            validate_group_selector_name(group_name.as_str())?;
            RelayHostSelector::Group(group_name)
        }
    };
    Ok(parsed)
}

fn validate_group_selector_name(group_name: &str) -> Result<(), RuntimeError> {
    if group_name == RESERVED_GROUP_ALL {
        return Ok(());
    }
    if is_custom_group_name(group_name) {
        return Ok(());
    }
    if is_reserved_group_name(group_name) {
        return Err(RuntimeError::validation(
            "validation_invalid_group_name",
            format!(
                "group '{}' is reserved; only '{}' is currently supported",
                group_name, RESERVED_GROUP_ALL
            ),
        ));
    }
    Err(RuntimeError::validation(
        "validation_invalid_group_name",
        format!(
            "group '{}' must be lowercase (custom) or '{}'",
            group_name, RESERVED_GROUP_ALL
        ),
    ))
}

fn is_reserved_group_name(group_name: &str) -> bool {
    group_name.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
    })
}

fn is_custom_group_name(group_name: &str) -> bool {
    group_name.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    })
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
        ConfigurationError::InvalidGroupName { path, group_name } => RuntimeError::validation(
            "validation_invalid_group_name",
            format!(
                "invalid group '{}' in bundle configuration {}",
                group_name,
                path.display()
            ),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => RuntimeError::validation(
            "validation_reserved_group_name",
            format!(
                "group '{}' is reserved in bundle configuration {}",
                group_name,
                path.display()
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

fn remove_relay_socket_file(socket_path: &Path) -> Result<(), RuntimeError> {
    match fs::remove_file(socket_path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RuntimeError::io(
            format!("remove relay socket {}", socket_path.display()),
            source,
        )),
    }
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
        "Usage: agentmux <command> [options]\n\nCommands:\n  host relay (<bundle-id> | --group GROUP) [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  list [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]\n  send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--quiescence-timeout-ms MS] [--request-id ID] [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn print_host_help() {
    println!("Usage: agentmux host <relay|mcp> [options]");
}

fn print_host_relay_help() {
    println!(
        "Usage: agentmux host relay (<bundle-id> | --group GROUP) [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
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
