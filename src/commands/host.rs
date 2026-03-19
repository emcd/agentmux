use std::{env, path::Path, thread, time::Duration};

use serde_json::{Map, Value, json};

use crate::{
    configuration::{RESERVED_GROUP_ALL, load_bundle_configuration, load_bundle_group_memberships},
    mcp::McpConfiguration,
    relay::{reconcile_bundle, shutdown_bundle_runtime, wait_for_async_delivery_shutdown},
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

use super::{
    McpHostArguments, RelayHostArguments, RelayHostSelector, RelayHostStartupBundle,
    RelayHostStartupSummary, RuntimeArguments, shared,
};

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
    .map_err(shared::map_reconcile_error)?;
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
    shared::remove_relay_socket_file(&paths.relay_socket)?;
    let shutdown =
        shutdown_bundle_runtime(&paths.tmux_socket).map_err(shared::map_reconcile_error)?;
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
    let memberships = load_bundle_group_memberships(&roots.configuration_root)
        .map_err(shared::map_bundle_load_error)?;
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
    let (reason_code, reason) = shared::runtime_error_reason(&source);
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some(reason_code),
        reason: Some(reason),
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
        selector: RelayHostSelector::Bundle(String::new()),
        runtime: RuntimeArguments::default(),
    };
    let mut positional_bundle = None::<String>;
    let mut group_name = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--group" => group_name = Some(shared::take_value(arguments, &mut index, "--group")?),
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
        "Usage: agentmux host relay (<bundle-id> | --group GROUP) [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

pub(super) fn print_host_mcp_help() {
    println!(
        "Usage: agentmux host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
