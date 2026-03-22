use std::{
    env,
    io::Write,
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

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

#[derive(Debug)]
struct RelayListenerWorker {
    paths: BundleRuntimePaths,
    join_handle: thread::JoinHandle<()>,
}

#[derive(Debug)]
struct RelayConnectionWorker {
    sender: mpsc::SyncSender<UnixStream>,
    join_handle: thread::JoinHandle<()>,
}

#[derive(Debug)]
struct RelayConnectionPoolMetrics {
    queued_connections: std::sync::atomic::AtomicUsize,
    active_connections: std::sync::atomic::AtomicUsize,
    rejected_connections: std::sync::atomic::AtomicUsize,
}

impl RelayConnectionPoolMetrics {
    fn new() -> Self {
        Self {
            queued_connections: std::sync::atomic::AtomicUsize::new(0),
            active_connections: std::sync::atomic::AtomicUsize::new(0),
            rejected_connections: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

enum RelayConnectionDispatchOutcome {
    Queued,
    QueueFull(UnixStream),
    WorkersUnavailable(UnixStream),
}

const RELAY_CONNECTION_WORKER_MIN: usize = 2;
const RELAY_CONNECTION_WORKER_MAX: usize = 8;
const RELAY_CONNECTION_QUEUE_CAPACITY: usize = 64;

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

    let stop_requested = Arc::new(AtomicBool::new(false));
    let (error_sender, error_receiver) = mpsc::channel::<RuntimeError>();
    let mut workers = hosted_bundles
        .into_iter()
        .map(|hosted_bundle| {
            spawn_relay_listener_worker(
                roots.configuration_root.clone(),
                hosted_bundle,
                Arc::clone(&stop_requested),
                error_sender.clone(),
            )
        })
        .collect::<Vec<_>>();
    drop(error_sender);

    let mut accept_error = None::<RuntimeError>;
    while !shutdown_requested() && !stop_requested.load(Ordering::SeqCst) {
        match error_receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(source) => {
                stop_requested.store(true, Ordering::SeqCst);
                accept_error = Some(source);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if shutdown_requested() {
        emit_inscription("relay.shutdown.signal", &json!({"signal": "termination"}));
    }
    stop_requested.store(true, Ordering::SeqCst);
    for worker in &workers {
        wake_listener(worker.paths.relay_socket.as_path());
    }
    let mut cleanup_paths = Vec::<BundleRuntimePaths>::with_capacity(workers.len());
    for worker in workers.drain(..) {
        cleanup_paths.push(worker.paths.clone());
        if worker.join_handle.join().is_err() && accept_error.is_none() {
            accept_error = Some(RuntimeError::validation(
                "internal_unexpected_failure",
                format!(
                    "relay listener worker panicked for bundle {}",
                    worker.paths.bundle_name
                ),
            ));
        }
    }

    let async_workers_remaining = if shutdown_requested() {
        wait_for_async_delivery_shutdown(Duration::from_millis(1_500))
    } else {
        0
    };
    for paths in cleanup_paths {
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
    }
    if let Some(error) = accept_error {
        return Err(error);
    }
    Ok(())
}

fn spawn_relay_listener_worker(
    configuration_root: std::path::PathBuf,
    hosted_bundle: HostedRelayBundle,
    stop_requested: Arc<AtomicBool>,
    error_sender: mpsc::Sender<RuntimeError>,
) -> RelayListenerWorker {
    let paths = hosted_bundle.paths.clone();
    let join_handle = thread::spawn(move || {
        run_relay_listener_worker(
            configuration_root,
            hosted_bundle,
            stop_requested,
            error_sender,
        );
    });
    RelayListenerWorker { paths, join_handle }
}

fn run_relay_listener_worker(
    configuration_root: std::path::PathBuf,
    hosted_bundle: HostedRelayBundle,
    stop_requested: Arc<AtomicBool>,
    error_sender: mpsc::Sender<RuntimeError>,
) {
    let metrics = Arc::new(RelayConnectionPoolMetrics::new());
    let mut connection_workers = spawn_relay_connection_worker_pool(
        configuration_root.clone(),
        hosted_bundle.paths.clone(),
        Arc::clone(&stop_requested),
        Arc::clone(&metrics),
    );
    let mut next_worker_index = 0usize;
    while !shutdown_requested() && !stop_requested.load(Ordering::SeqCst) {
        match hosted_bundle.listener.accept() {
            Ok((stream, _)) => {
                if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
                    break;
                }
                match dispatch_connection_to_worker_pool(
                    &connection_workers,
                    &mut next_worker_index,
                    stream,
                    &metrics,
                ) {
                    RelayConnectionDispatchOutcome::Queued => {}
                    RelayConnectionDispatchOutcome::QueueFull(stream) => {
                        reject_overloaded_connection(&hosted_bundle.paths, stream, &metrics);
                    }
                    RelayConnectionDispatchOutcome::WorkersUnavailable(stream) => {
                        let _ = stream.shutdown(Shutdown::Both);
                        let _ = error_sender.send(RuntimeError::validation(
                            "internal_unexpected_failure",
                            format!(
                                "relay connection workers unavailable for bundle {}",
                                hosted_bundle.paths.bundle_name
                            ),
                        ));
                        break;
                    }
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => {
                let _ = error_sender.send(RuntimeError::io(
                    format!(
                        "accept relay socket connection for bundle {}",
                        hosted_bundle.paths.bundle_name
                    ),
                    source,
                ));
                break;
            }
        }
    }
    if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
        emit_inscription(
            "relay.shutdown.connection_workers_detached",
            &json!({
                "bundle_name": hosted_bundle.paths.bundle_name,
                "connection_worker_count": connection_workers.len(),
                "queued_connections": metrics
                    .queued_connections
                    .load(Ordering::SeqCst),
                "active_connections": metrics
                    .active_connections
                    .load(Ordering::SeqCst),
                "rejected_connections": metrics
                    .rejected_connections
                    .load(Ordering::SeqCst),
            }),
        );
        return;
    }
    for worker in connection_workers.drain(..) {
        if worker.join_handle.join().is_err() {
            let _ = error_sender.send(RuntimeError::validation(
                "internal_unexpected_failure",
                format!(
                    "relay connection worker panicked for bundle {}",
                    hosted_bundle.paths.bundle_name
                ),
            ));
            break;
        }
    }
}

fn relay_connection_worker_count() -> usize {
    if let Some(override_count) = parse_env_positive_usize("AGENTMUX_RELAY_CONNECTION_WORKERS") {
        return override_count;
    }
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(RELAY_CONNECTION_WORKER_MIN)
        .clamp(RELAY_CONNECTION_WORKER_MIN, RELAY_CONNECTION_WORKER_MAX)
}

fn relay_connection_queue_capacity() -> usize {
    parse_env_positive_usize("AGENTMUX_RELAY_CONNECTION_QUEUE_CAPACITY")
        .unwrap_or(RELAY_CONNECTION_QUEUE_CAPACITY)
}

fn parse_env_positive_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn spawn_relay_connection_worker_pool(
    configuration_root: std::path::PathBuf,
    bundle_paths: BundleRuntimePaths,
    stop_requested: Arc<AtomicBool>,
    metrics: Arc<RelayConnectionPoolMetrics>,
) -> Vec<RelayConnectionWorker> {
    let worker_count = relay_connection_worker_count();
    (0..worker_count)
        .map(|_| {
            spawn_relay_connection_worker(
                configuration_root.clone(),
                bundle_paths.clone(),
                Arc::clone(&stop_requested),
                Arc::clone(&metrics),
            )
        })
        .collect::<Vec<_>>()
}

fn spawn_relay_connection_worker(
    configuration_root: std::path::PathBuf,
    bundle_paths: BundleRuntimePaths,
    stop_requested: Arc<AtomicBool>,
    metrics: Arc<RelayConnectionPoolMetrics>,
) -> RelayConnectionWorker {
    let (sender, receiver) = mpsc::sync_channel::<UnixStream>(relay_connection_queue_capacity());
    let join_handle = thread::spawn(move || {
        run_relay_connection_worker(
            configuration_root,
            bundle_paths,
            stop_requested,
            receiver,
            metrics,
        );
    });
    RelayConnectionWorker {
        sender,
        join_handle,
    }
}

fn run_relay_connection_worker(
    configuration_root: std::path::PathBuf,
    bundle_paths: BundleRuntimePaths,
    stop_requested: Arc<AtomicBool>,
    receiver: mpsc::Receiver<UnixStream>,
    metrics: Arc<RelayConnectionPoolMetrics>,
) {
    loop {
        if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(mut stream) => {
                metrics.queued_connections.fetch_sub(1, Ordering::SeqCst);
                metrics.active_connections.fetch_add(1, Ordering::SeqCst);
                if let Err(source) =
                    crate::relay::serve_connection(&mut stream, &configuration_root, &bundle_paths)
                {
                    emit_inscription(
                        "relay.request_failed",
                        &json!({
                            "bundle_name": bundle_paths.bundle_name,
                            "error": source.to_string(),
                        }),
                    );
                    eprintln!("agentmux host relay: request handling failed: {source}");
                }
                metrics.active_connections.fetch_sub(1, Ordering::SeqCst);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn dispatch_connection_to_worker_pool(
    connection_workers: &[RelayConnectionWorker],
    next_worker_index: &mut usize,
    mut stream: UnixStream,
    metrics: &RelayConnectionPoolMetrics,
) -> RelayConnectionDispatchOutcome {
    if connection_workers.is_empty() {
        return RelayConnectionDispatchOutcome::WorkersUnavailable(stream);
    }
    let worker_count = connection_workers.len();
    let mut saw_queue_full = false;
    let mut saw_disconnected = false;
    for offset in 0..worker_count {
        let worker_index = (*next_worker_index + offset) % worker_count;
        match connection_workers[worker_index].sender.try_send(stream) {
            Ok(()) => {
                metrics.queued_connections.fetch_add(1, Ordering::SeqCst);
                *next_worker_index = (worker_index + 1) % worker_count;
                return RelayConnectionDispatchOutcome::Queued;
            }
            Err(mpsc::TrySendError::Full(returned_stream)) => {
                saw_queue_full = true;
                stream = returned_stream;
            }
            Err(mpsc::TrySendError::Disconnected(returned_stream)) => {
                saw_disconnected = true;
                stream = returned_stream;
            }
        }
    }
    if saw_queue_full {
        return RelayConnectionDispatchOutcome::QueueFull(stream);
    }
    if saw_disconnected {
        return RelayConnectionDispatchOutcome::WorkersUnavailable(stream);
    }
    RelayConnectionDispatchOutcome::WorkersUnavailable(stream)
}

fn reject_overloaded_connection(
    bundle_paths: &BundleRuntimePaths,
    mut stream: UnixStream,
    metrics: &RelayConnectionPoolMetrics,
) {
    metrics.rejected_connections.fetch_add(1, Ordering::SeqCst);
    emit_inscription(
        "relay.connection.rejected",
        &json!({
            "bundle_name": bundle_paths.bundle_name,
            "reason_code": "runtime_connection_queue_full",
            "reason": "connection worker pool queue is full",
        }),
    );
    let response = crate::relay::RelayResponse::Error {
        error: crate::relay::RelayError {
            code: "runtime_connection_queue_full".to_string(),
            message: "relay connection worker pool queue is full".to_string(),
            details: Some(json!({
                "bundle_name": bundle_paths.bundle_name,
            })),
        },
    };
    if let Ok(mut encoded) = serde_json::to_vec(&response) {
        encoded.push(b'\n');
        let _ = stream.write_all(&encoded);
        let _ = stream.flush();
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn wake_listener(socket_path: &Path) {
    let _ = UnixStream::connect(socket_path);
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
