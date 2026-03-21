use std::{env, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Map, Value, json};

use crate::{
    configuration::{load_bundle_configuration, load_bundle_group_memberships},
    mcp::McpConfiguration,
    relay::{reconcile_bundle, shutdown_bundle_runtime, wait_for_async_delivery_shutdown},
    runtime::{
        association::{
            McpAssociationCli, WorkspaceContext, load_local_mcp_overrides, resolve_association,
            resolve_sender_session,
        },
        bootstrap::{
            RelayRuntimeLock, acquire_relay_runtime_lock, bind_relay_listener,
            relay_runtime_lock_is_held,
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

use super::{
    McpHostArguments, RelayHostArguments, RelayHostStartupBundle, RelayHostStartupSummary,
    RuntimeArguments, shared,
};

#[derive(Clone, Debug)]
enum RelayHostStartupMode {
    Autostart,
    ProcessOnly,
}

#[derive(Debug)]
struct HostedRelayBundle {
    paths: BundleRuntimePaths,
    listener: UnixListener,
    _runtime_lock: RelayRuntimeLock,
}

pub(super) async fn run_agentmux_host(arguments: &[String]) -> Result<(), RuntimeError> {
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

fn run_relay_host(arguments: RelayHostArguments) -> Result<(), RuntimeError> {
    let roots = resolve_runtime_roots(arguments.runtime)?;
    run_relay_host_no_selector(&roots, arguments.no_autostart)
}

fn resolve_runtime_roots(runtime: RuntimeArguments) -> Result<RuntimeRoots, RuntimeError> {
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let overrides = RuntimeRootOverrides {
        configuration_root: runtime.configuration_root,
        state_root: runtime.state_root,
        inscriptions_root: runtime.inscriptions_root,
        repository_root: runtime.repository_root.or(Some(current_directory)),
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    Ok(roots)
}

fn run_relay_host_no_selector(
    roots: &RuntimeRoots,
    no_autostart: bool,
) -> Result<(), RuntimeError> {
    let memberships = load_bundle_group_memberships(&roots.configuration_root)
        .map_err(shared::map_bundle_load_error)?;
    if let Some(first_bundle) = memberships.first()
        && let Err(source) = configure_process_inscriptions(&relay_inscriptions_path(
            &roots.inscriptions_root,
            first_bundle.bundle_name.as_str(),
        ))
    {
        return Err(source);
    }
    let mut outcomes = Vec::with_capacity(memberships.len());
    let mut hosted_bundles = Vec::<HostedRelayBundle>::with_capacity(memberships.len());
    for membership in memberships {
        let startup_mode = if no_autostart || !membership.autostart {
            RelayHostStartupMode::ProcessOnly
        } else {
            RelayHostStartupMode::Autostart
        };
        let (outcome, hosted_bundle) =
            host_selected_bundle(roots, membership.bundle_name.as_str(), startup_mode);
        outcomes.push(outcome);
        if let Some(hosted_bundle) = hosted_bundle {
            hosted_bundles.push(hosted_bundle);
        }
    }

    let summary = build_startup_summary(
        if no_autostart {
            "process_only"
        } else {
            "autostart"
        },
        outcomes,
    );
    if hosted_bundles.is_empty() {
        if summary.failed_bundle_count > 0 {
            return Err(RuntimeError::validation(
                "runtime_startup_failed",
                format!(
                    "failed to start relay for {} bundle(s)",
                    summary.failed_bundle_count
                ),
            ));
        }
        return Ok(());
    }

    let _signal_handlers = install_shutdown_signal_handlers()?;
    for hosted_bundle in &hosted_bundles {
        println!(
            "agentmux host relay listening bundle={} socket={}",
            hosted_bundle.paths.bundle_name,
            hosted_bundle.paths.relay_socket.display(),
        );
    }
    emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
    render_startup_summary(&summary);

    let mut accept_error = None::<RuntimeError>;
    while !shutdown_requested() {
        let mut accepted_request = false;
        'bundle_accept: for hosted_bundle in &hosted_bundles {
            loop {
                match hosted_bundle.listener.accept() {
                    Ok((mut stream, _)) => {
                        accepted_request = true;
                        if let Err(source) = crate::relay::serve_connection(
                            &mut stream,
                            &roots.configuration_root,
                            &hosted_bundle.paths,
                        ) {
                            emit_inscription(
                                "relay.request_failed",
                                &json!({
                                    "bundle_name": hosted_bundle.paths.bundle_name,
                                    "error": source.to_string(),
                                }),
                            );
                            eprintln!("agentmux host relay: request handling failed: {source}");
                        }
                    }
                    Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(source) => {
                        accept_error = Some(RuntimeError::io(
                            format!(
                                "accept relay socket connection for bundle {}",
                                hosted_bundle.paths.bundle_name
                            ),
                            source,
                        ));
                        break 'bundle_accept;
                    }
                }
            }
        }
        if accept_error.is_some() {
            break;
        }
        if !accepted_request {
            thread::sleep(Duration::from_millis(50));
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
    while let Some(hosted_bundle) = hosted_bundles.pop() {
        drop(hosted_bundle.listener);
        shared::remove_relay_socket_file(&hosted_bundle.paths.relay_socket)?;
        let shutdown = shutdown_bundle_runtime(&hosted_bundle.paths.tmux_socket)
            .map_err(shared::map_reconcile_error)?;
        emit_inscription(
            "relay.shutdown.complete",
            &json!({
                "bundle_name": hosted_bundle.paths.bundle_name,
                "pruned_count": shutdown.pruned_sessions.len(),
                "killed_tmux_server": shutdown.killed_tmux_server,
                "pruned_sessions": shutdown.pruned_sessions,
                "async_workers_remaining": async_workers_remaining,
            }),
        );
    }
    if let Some(error) = accept_error {
        return Err(error);
    }
    Ok(())
}

fn host_selected_bundle(
    roots: &RuntimeRoots,
    bundle_name: &str,
    startup_mode: RelayHostStartupMode,
) -> (RelayHostStartupBundle, Option<HostedRelayBundle>) {
    let paths = match BundleRuntimePaths::resolve(&roots.state_root, bundle_name) {
        Ok(paths) => paths,
        Err(source) => return (failed_startup_bundle(bundle_name, source), None),
    };

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
    match relay_runtime_lock_is_held(&paths) {
        Ok(true) => {
            return (
                skipped_startup_bundle(
                    bundle_name,
                    "lock_held",
                    "relay runtime lock is already held".to_string(),
                ),
                None,
            );
        }
        Err(source) => return (failed_startup_bundle(bundle_name, source), None),
        Ok(false) => {}
    }
    if let Err(source) = ensure_bundle_runtime_directory(&paths) {
        return (failed_startup_bundle(bundle_name, source), None);
    }
    let runtime_lock = match acquire_relay_runtime_lock(&paths) {
        Ok(runtime_lock) => runtime_lock,
        Err(source) => {
            if matches!(
                &source,
                RuntimeError::Io {
                    source,
                    ..
                } if source.kind() == std::io::ErrorKind::WouldBlock
            ) {
                return (
                    skipped_startup_bundle(
                        bundle_name,
                        "lock_held",
                        "relay runtime lock is already held".to_string(),
                    ),
                    None,
                );
            }
            return (failed_startup_bundle(bundle_name, source), None);
        }
    };
    if let RelayHostStartupMode::Autostart = startup_mode
        && let Err(source) = reconcile_bundle(
            &roots.configuration_root,
            &paths.bundle_name,
            &paths.tmux_socket,
        )
        .map_err(shared::map_reconcile_error)
    {
        return (failed_startup_bundle(bundle_name, source), None);
    }
    let listener = match bind_relay_listener(&paths) {
        Ok(listener) => listener,
        Err(source) => return (failed_startup_bundle(bundle_name, source), None),
    };
    if let Err(source) = listener.set_nonblocking(true) {
        return (
            failed_startup_bundle(
                bundle_name,
                RuntimeError::io("set relay socket listener nonblocking", source),
            ),
            None,
        );
    }
    let startup_bundle = match startup_mode {
        RelayHostStartupMode::Autostart => hosted_startup_bundle(bundle_name),
        RelayHostStartupMode::ProcessOnly => skipped_startup_bundle(
            bundle_name,
            "process_only",
            "relay started without bundle autostart".to_string(),
        ),
    };
    (
        startup_bundle,
        Some(HostedRelayBundle {
            paths,
            listener,
            _runtime_lock: runtime_lock,
        }),
    )
}

fn build_startup_summary(
    host_mode: &str,
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
    println!(
        "agentmux host relay summary mode={} hosted={} skipped={} failed={} hosted_any={}",
        summary.host_mode,
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
    let (reason_code, reason) = shared::runtime_error_reason(&source);
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some(reason_code),
        reason: Some(reason),
    }
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
    let roots = shared::resolve_roots(&arguments.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(shared::map_bundle_load_error)?;
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
        no_autostart: false,
        runtime: RuntimeArguments::default(),
    };
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--no-autostart" => parsed.no_autostart = true,
            "--all" | "--include-bundle" | "--exclude-bundle" => {
                return Err(RuntimeError::validation(
                    "validation_invalid_arguments",
                    format!("'{}' is not supported in relay host MVP", arguments[index]),
                ));
            }
            value if !value.starts_with('-') => {
                return Err(RuntimeError::validation(
                    "validation_invalid_arguments",
                    format!("'{}' is not supported in relay host MVP", value),
                ));
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

fn parse_host_mcp_arguments(arguments: &[String]) -> Result<McpHostArguments, RuntimeError> {
    let mut parsed = McpHostArguments::default();
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
            "--session-name" => {
                parsed.session_name =
                    Some(shared::take_value(arguments, &mut index, "--session-name")?);
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

pub(super) fn print_host_help() {
    println!("Usage: agentmux host <relay|mcp> [options]");
}

pub(super) fn print_host_relay_help() {
    println!(
        "Usage: agentmux host relay [--no-autostart] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

pub(super) fn print_host_mcp_help() {
    println!(
        "Usage: agentmux host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
