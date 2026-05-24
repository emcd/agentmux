use std::{
    env,
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::json;
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener as TokioUnixListener, UnixStream as TokioUnixStream},
    sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError},
    task::JoinSet,
};

use crate::{
    configuration::load_bundle_group_memberships,
    relay::{
        append_startup_failure, serve_connection, shutdown_bundle_runtime, startup_bundle,
        wait_for_async_delivery_shutdown,
    },
    runtime::{
        bootstrap::{
            RelayRuntimeLock, acquire_relay_runtime_lock, bind_relay_listener,
            relay_runtime_lock_is_held,
        },
        error::RuntimeError,
        inscriptions::{configure_process_inscriptions, emit_inscription, relay_inscriptions_path},
        paths::{
            BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots, ensure_bundle_runtime_directory,
        },
        signals::{install_shutdown_signal_handlers, shutdown_requested},
        starter::ensure_starter_configuration_layout,
    },
};

use crate::commands::{
    RelayHostArguments, RelayHostStartupBundle, RelayHostStartupSummary, RuntimeArguments, shared,
};

use super::summary::{
    build_startup_summary, failed_startup_bundle, hosted_startup_bundle, render_startup_summary,
    skipped_startup_bundle, startup_summary_payload,
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

/// Outcome of the synchronous relay-host startup phase.
///
/// `NoHostedBundles` means startup finalized without anything to serve (process
/// only, or zero ready bundles) and the summary, if any, has already been
/// emitted. `Serve` carries the bound listeners forward into the async serve
/// phase.
enum RelayHostPreparation {
    NoHostedBundles,
    Serve {
        summary: RelayHostStartupSummary,
        hosted_bundles: Vec<HostedRelayBundle>,
    },
}

#[derive(Debug)]
struct RelayConnectionMetrics {
    active_connections: AtomicUsize,
    rejected_connections: AtomicUsize,
}

impl RelayConnectionMetrics {
    fn new() -> Self {
        Self {
            active_connections: AtomicUsize::new(0),
            rejected_connections: AtomicUsize::new(0),
        }
    }
}

// Unified cap on concurrent relay connections (persistent streams and
// short-lived requests share it), enforced by a semaphore at accept time.
// Raise via AGENTMUX_RELAY_MAX_CONNECTIONS if multi-tenant load ever demands it.
const RELAY_MAX_CONNECTIONS: usize = 512;
const RELAY_PRE_HELLO_IDLE_TIMEOUT_MS: u64 = 2_000;
const RELAY_SHUTDOWN_POLL_INTERVAL_MS: u64 = 100;

pub(super) async fn run_relay_host(arguments: RelayHostArguments) -> Result<(), RuntimeError> {
    // Startup (config load, tmux autostart, lock acquisition, socket binding) is
    // blocking, so it runs on a blocking task. The async serve phase then drives
    // the per-bundle accept loops on the runtime (tokio::net + tokio::spawn).
    let (roots, preparation) = tokio::task::spawn_blocking(move || {
        let roots = resolve_runtime_roots(arguments.runtime)?;
        let preparation = prepare_relay_host(&roots, arguments.no_autostart)?;
        Ok::<_, RuntimeError>((roots, preparation))
    })
    .await
    .map_err(|source| supervisor_join_error("relay host startup", source))??;

    match preparation {
        RelayHostPreparation::NoHostedBundles => Ok(()),
        RelayHostPreparation::Serve {
            summary,
            hosted_bundles,
        } => serve_relay_host(roots, summary, hosted_bundles).await,
    }
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

/// Runs the synchronous startup phase: load memberships, host each bundle, and
/// decide whether there is anything to serve.
fn prepare_relay_host(
    roots: &RuntimeRoots,
    no_autostart: bool,
) -> Result<RelayHostPreparation, RuntimeError> {
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
        let (mut outcome, hosted_bundle) =
            host_selected_bundle(roots, membership.bundle_name.as_str(), startup_mode);
        if no_autostart {
            outcome = skipped_startup_bundle(
                membership.bundle_name.as_str(),
                "process_only",
                "relay started without bundle autostart".to_string(),
            );
        }
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
        if no_autostart {
            emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
            render_startup_summary(&summary);
            return Ok(RelayHostPreparation::NoHostedBundles);
        }
        if summary.failed_bundle_count > 0 {
            return Err(RuntimeError::validation(
                "runtime_startup_failed",
                format!(
                    "failed to start relay for {} bundle(s)",
                    summary.failed_bundle_count
                ),
            ));
        }
        return Ok(RelayHostPreparation::NoHostedBundles);
    }

    Ok(RelayHostPreparation::Serve {
        summary,
        hosted_bundles,
    })
}

/// Drives the per-bundle accept loops on the async runtime until shutdown, then
/// performs runtime cleanup.
async fn serve_relay_host(
    roots: RuntimeRoots,
    summary: RelayHostStartupSummary,
    hosted_bundles: Vec<HostedRelayBundle>,
) -> Result<(), RuntimeError> {
    let _signal_handlers = install_shutdown_signal_handlers()?;
    emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
    render_startup_summary(&summary);

    let stop_requested = Arc::new(AtomicBool::new(false));
    let max_connections = relay_max_connections();
    let connection_permits = Arc::new(Semaphore::new(max_connections));

    let mut accept_tasks: JoinSet<Result<(), RuntimeError>> = JoinSet::new();
    let mut cleanup_paths = Vec::<BundleRuntimePaths>::with_capacity(hosted_bundles.len());
    for hosted_bundle in hosted_bundles {
        cleanup_paths.push(hosted_bundle.paths.clone());
        accept_tasks.spawn(run_relay_accept_loop(
            roots.configuration_root.clone(),
            hosted_bundle,
            Arc::clone(&stop_requested),
            Arc::clone(&connection_permits),
            max_connections,
        ));
    }

    let mut accept_error = supervise_accept_loops(&stop_requested, &mut accept_tasks).await;

    if shutdown_requested() {
        emit_inscription("relay.shutdown.signal", &json!({"signal": "termination"}));
    }
    stop_requested.store(true, Ordering::SeqCst);

    // In-flight connection tasks are intentionally detached on shutdown (as the
    // connection-worker pool was before); only the accept loops are joined here.
    while let Some(joined) = accept_tasks.join_next().await {
        if let Some(error) = accept_loop_join_error(joined)
            && accept_error.is_none()
        {
            accept_error = Some(error);
        }
    }

    // Cleanup (async-delivery drain, socket removal, tmux shutdown) is blocking.
    tokio::task::spawn_blocking(move || cleanup_relay_host(cleanup_paths))
        .await
        .map_err(|source| supervisor_join_error("relay host cleanup", source))??;

    if let Some(error) = accept_error {
        return Err(error);
    }
    Ok(())
}

/// Waits until a shutdown signal arrives or an accept loop fails. A clean accept
/// loop exit (graceful stop) is tolerated; a failure stops the remaining loops
/// and is surfaced to the caller.
async fn supervise_accept_loops(
    stop_requested: &Arc<AtomicBool>,
    accept_tasks: &mut JoinSet<Result<(), RuntimeError>>,
) -> Option<RuntimeError> {
    loop {
        if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
            return None;
        }
        tokio::select! {
            biased;
            joined = accept_tasks.join_next() => {
                match joined {
                    None => return None,
                    Some(result) => {
                        if let Some(error) = accept_loop_join_error(result) {
                            stop_requested.store(true, Ordering::SeqCst);
                            return Some(error);
                        }
                    }
                }
            }
            () = tokio::time::sleep(Duration::from_millis(RELAY_SHUTDOWN_POLL_INTERVAL_MS)) => {}
        }
    }
}

/// Maps an accept-loop task's join result to an optional fatal error.
fn accept_loop_join_error(
    result: Result<Result<(), RuntimeError>, tokio::task::JoinError>,
) -> Option<RuntimeError> {
    match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(join_error) => Some(RuntimeError::validation(
            "internal_unexpected_failure",
            format!("relay accept loop task panicked: {join_error}"),
        )),
    }
}

/// Builds a runtime error for a blocking supervisor/cleanup task that failed to
/// join (panic or cancellation).
fn supervisor_join_error(context: &str, source: tokio::task::JoinError) -> RuntimeError {
    RuntimeError::validation(
        "internal_unexpected_failure",
        format!("{context} task failed to join: {source}"),
    )
}

/// Performs runtime cleanup after the accept loops have stopped: drains async
/// delivery workers (on shutdown), removes relay sockets, and shuts down tmux.
fn cleanup_relay_host(cleanup_paths: Vec<BundleRuntimePaths>) -> Result<(), RuntimeError> {
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
    Ok(())
}

/// Accepts connections for one bundle and spawns a task per connection, bounded
/// by the shared connection semaphore. At the cap, new connections receive an
/// immediate overloaded response.
async fn run_relay_accept_loop(
    configuration_root: std::path::PathBuf,
    hosted_bundle: HostedRelayBundle,
    stop_requested: Arc<AtomicBool>,
    connection_permits: Arc<Semaphore>,
    max_connections: usize,
) -> Result<(), RuntimeError> {
    // `_runtime_lock` is held for the loop's lifetime (released on task exit) to
    // keep the bundle's relay runtime lock owned while the socket is served.
    let HostedRelayBundle {
        paths,
        listener,
        _runtime_lock,
    } = hosted_bundle;
    listener.set_nonblocking(true).map_err(|source| {
        RuntimeError::io(
            format!(
                "set relay socket non-blocking for bundle {}",
                paths.bundle_name
            ),
            source,
        )
    })?;
    let listener = TokioUnixListener::from_std(listener).map_err(|source| {
        RuntimeError::io(
            format!(
                "register relay socket with async runtime for bundle {}",
                paths.bundle_name
            ),
            source,
        )
    })?;
    let metrics = Arc::new(RelayConnectionMetrics::new());

    let accept_outcome = loop {
        if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
            break Ok(());
        }
        tokio::select! {
            biased;
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _address)) => {
                        if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
                            break Ok(());
                        }
                        match Arc::clone(&connection_permits).try_acquire_owned() {
                            Ok(permit) => {
                                spawn_connection_worker(
                                    permit,
                                    stream,
                                    configuration_root.clone(),
                                    paths.clone(),
                                    Arc::clone(&metrics),
                                );
                            }
                            Err(TryAcquireError::NoPermits) => {
                                reject_overloaded_connection(&paths, stream, &metrics).await;
                            }
                            Err(TryAcquireError::Closed) => break Ok(()),
                        }
                        emit_connection_metrics(&paths, max_connections, &metrics);
                    }
                    Err(source) => {
                        break Err(RuntimeError::io(
                            format!(
                                "accept relay socket connection for bundle {}",
                                paths.bundle_name
                            ),
                            source,
                        ));
                    }
                }
            }
            () = tokio::time::sleep(Duration::from_millis(RELAY_SHUTDOWN_POLL_INTERVAL_MS)) => {}
        }
    };

    if accept_outcome.is_ok() && (shutdown_requested() || stop_requested.load(Ordering::SeqCst)) {
        emit_inscription(
            "relay.shutdown.connection_workers_detached",
            &json!({
                "bundle_name": paths.bundle_name,
                "max_connections": max_connections,
                "active_connections": metrics.active_connections.load(Ordering::SeqCst),
                "rejected_connections": metrics.rejected_connections.load(Ordering::SeqCst),
            }),
        );
    }
    accept_outcome
}

/// Spawns a tokio task to serve one accepted connection asynchronously. The
/// task is detached: in-flight tasks are cancelled when the runtime is dropped
/// (process shutdown), which triggers the connection's drop-guard unregister
/// path. Per-connection writes run on a separate writer task spawned inside
/// `serve_connection`, so no blocking call ever ties up a runtime worker.
fn spawn_connection_worker(
    permit: OwnedSemaphorePermit,
    stream: TokioUnixStream,
    configuration_root: std::path::PathBuf,
    paths: BundleRuntimePaths,
    metrics: Arc<RelayConnectionMetrics>,
) {
    metrics.active_connections.fetch_add(1, Ordering::SeqCst);
    let pre_hello_idle_timeout = relay_pre_hello_idle_timeout();
    tokio::spawn(async move {
        let result =
            serve_connection(stream, &configuration_root, &paths, pre_hello_idle_timeout).await;
        metrics.active_connections.fetch_sub(1, Ordering::SeqCst);
        drop(permit);
        if let Err(source) = result {
            emit_inscription(
                "relay.request_failed",
                &json!({
                    "bundle_name": paths.bundle_name,
                    "error": source.to_string(),
                }),
            );
        }
    });
}

fn relay_max_connections() -> usize {
    parse_env_positive_usize("AGENTMUX_RELAY_MAX_CONNECTIONS").unwrap_or(RELAY_MAX_CONNECTIONS)
}

fn relay_pre_hello_idle_timeout() -> Duration {
    let timeout_ms = parse_env_positive_usize("AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS")
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(RELAY_PRE_HELLO_IDLE_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

fn parse_env_positive_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

async fn reject_overloaded_connection(
    paths: &BundleRuntimePaths,
    mut stream: TokioUnixStream,
    metrics: &RelayConnectionMetrics,
) {
    metrics.rejected_connections.fetch_add(1, Ordering::SeqCst);
    emit_inscription(
        "relay.connection.rejected",
        &json!({
            "bundle_name": paths.bundle_name,
            "reason_code": "runtime_connection_limit_reached",
            "reason": "relay connection limit reached",
        }),
    );
    let response = crate::relay::RelayResponse::Error {
        error: crate::relay::RelayError {
            code: "runtime_connection_limit_reached".to_string(),
            message: "relay connection limit reached".to_string(),
            details: Some(json!({
                "bundle_name": paths.bundle_name,
            })),
        },
    };
    let frame = json!({
        "frame": "response",
        "response": response,
    });
    if let Ok(mut encoded) = serde_json::to_vec(&frame) {
        encoded.push(b'\n');
        let _ = stream.write_all(&encoded).await;
        let _ = stream.flush().await;
    }
    let _ = stream.shutdown().await;
}

// Emits a snapshot of connection occupancy on each accept so saturation is
// observable: `active_connections` approaching `max_connections` with non-zero
// `rejected_connections` is the at-capacity signal.
fn emit_connection_metrics(
    paths: &BundleRuntimePaths,
    max_connections: usize,
    metrics: &RelayConnectionMetrics,
) {
    emit_inscription(
        "relay.connection_pool.metrics",
        &json!({
            "bundle_name": paths.bundle_name,
            "max_connections": max_connections,
            "active_connections": metrics.active_connections.load(Ordering::SeqCst),
            "rejected_connections": metrics.rejected_connections.load(Ordering::SeqCst),
        }),
    );
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
    let mut startup_report = None;
    if let RelayHostStartupMode::Autostart = startup_mode {
        let report = match startup_bundle(
            &roots.configuration_root,
            &paths.bundle_name,
            &paths.runtime_directory,
        )
        .map_err(shared::map_reconcile_error)
        {
            Ok(report) => report,
            Err(source) => return (failed_startup_bundle(bundle_name, source), None),
        };
        for failure in &report.failed_startups {
            let persisted = match append_startup_failure(&paths.runtime_directory, failure.clone())
            {
                Ok(value) => value,
                Err(cause) => {
                    return (
                        RelayHostStartupBundle {
                            bundle_name: bundle_name.to_string(),
                            outcome: "failed".to_string(),
                            reason_code: Some("runtime_startup_failed".to_string()),
                            reason: Some(format!(
                                "failed to persist startup failure history: {cause}"
                            )),
                        },
                        None,
                    );
                }
            };
            emit_inscription(
                "relay.session_start_failed",
                &json!({
                    "bundle_name": persisted.bundle_name,
                    "session_id": persisted.session_id,
                    "transport": persisted.transport,
                    "code": persisted.code,
                    "reason": persisted.reason,
                    "timestamp": persisted.timestamp,
                    "sequence": persisted.sequence,
                    "details": persisted.details,
                }),
            );
        }
        startup_report = Some(report);
    }
    let listener = match bind_relay_listener(&paths) {
        Ok(listener) => listener,
        Err(source) => return (failed_startup_bundle(bundle_name, source), None),
    };
    let startup_bundle = match startup_mode {
        RelayHostStartupMode::Autostart => {
            let report = startup_report.expect("autostart report must be present");
            if report.ready_session_count > 0 {
                hosted_startup_bundle(bundle_name)
            } else {
                RelayHostStartupBundle {
                    bundle_name: bundle_name.to_string(),
                    outcome: "failed".to_string(),
                    reason_code: Some("runtime_startup_failed".to_string()),
                    reason: Some("zero configured sessions reached ready state".to_string()),
                }
            }
        }
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
