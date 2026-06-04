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
};

use crate::{
    configuration::load_bundle_group_memberships,
    relay::{
        BundleCatalog, append_startup_failure, serve_connection, shutdown_bundle_runtime,
        spawn_bundle_watcher, startup_bundle, wait_for_async_delivery_shutdown,
    },
    runtime::{
        bootstrap::{
            RelayRuntimeLock, acquire_relay_runtime_lock, bind_relay_listener,
            relay_runtime_lock_is_held,
        },
        error::RuntimeError,
        inscriptions::{configure_process_inscriptions, emit_inscription, relay_inscriptions_path},
        paths::{
            BundleRuntimePaths, RelayRuntimePaths, RuntimeRootOverrides, RuntimeRoots,
            ensure_bundle_runtime_directory, ensure_relay_runtime_directory,
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
struct HostedBundle {
    paths: BundleRuntimePaths,
}

/// Outcome of the synchronous relay-host startup phase.
///
/// `NoHostedBundles` means startup finalized without anything to serve (process
/// only, or zero ready bundles) and the summary, if any, has already been
/// emitted. `Serve` carries the bound listener forward into the async serve
/// phase.
enum RelayHostPreparation {
    NoHostedBundles,
    Serve(Box<RelayHostServePlan>),
}

#[derive(Debug)]
struct RelayHostServePlan {
    summary: RelayHostStartupSummary,
    hosted_bundles: Vec<HostedBundle>,
    relay_paths: RelayRuntimePaths,
    listener: UnixListener,
    runtime_lock: RelayRuntimeLock,
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
    // Install SIGINT/SIGTERM handlers before any startup work. The handlers do
    // nothing beyond flipping a process-wide atomic, so they tolerate being
    // installed before the runtime is fully initialized. Installing them here
    // (rather than inside `serve_relay_host`) closes a window where a SIGINT
    // delivered between socket bind and handler install would terminate the
    // relay with default signal disposition. This was observed as macOS CI
    // flakiness for relay_sigint_* tests (issues/relay/20): the test gated on
    // socket existence, which on macOS could become true before the handlers
    // were installed.
    let _signal_handlers = install_shutdown_signal_handlers()?;

    // Captured before `arguments` is moved into the blocking startup closure;
    // enforcement and watching apply in the async serve phase, not during startup.
    let require_session_credentials = arguments.require_session_credentials;
    let watch_bundles = arguments.watch_bundles;

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
        RelayHostPreparation::Serve(plan) => {
            let RelayHostServePlan {
                summary,
                hosted_bundles,
                relay_paths,
                listener,
                runtime_lock,
            } = *plan;
            serve_relay_host(
                roots,
                summary,
                hosted_bundles,
                relay_paths,
                listener,
                runtime_lock,
                require_session_credentials,
                watch_bundles,
            )
            .await
        }
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

/// Runs the synchronous startup phase: acquire the relay-level lock, bind
/// the single relay socket, host each bundle, and decide whether there is
/// anything to serve.
fn prepare_relay_host(
    roots: &RuntimeRoots,
    no_autostart: bool,
) -> Result<RelayHostPreparation, RuntimeError> {
    let memberships = load_bundle_group_memberships(&roots.configuration_root)
        .map_err(shared::map_bundle_load_error)?;
    configure_process_inscriptions(&relay_inscriptions_path(&roots.inscriptions_root))?;

    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    ensure_relay_runtime_directory(&relay_paths)?;

    // Acquire the relay-level runtime lock before any per-bundle startup work.
    // Holding it for the rest of the host lifetime serializes relay instances
    // against this state root.
    if relay_runtime_lock_is_held(&relay_paths)? {
        // Another relay already owns the socket for this state root. Render
        // an all-skipped summary and exit without binding.
        let outcomes = memberships
            .into_iter()
            .map(|membership| {
                skipped_startup_bundle(
                    membership.bundle_name.as_str(),
                    "lock_held",
                    "relay runtime lock is already held".to_string(),
                )
            })
            .collect();
        let summary = build_startup_summary(
            if no_autostart {
                "process_only"
            } else {
                "autostart"
            },
            outcomes,
        );
        emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
        render_startup_summary(&summary);
        return Ok(RelayHostPreparation::NoHostedBundles);
    }
    let runtime_lock = acquire_relay_runtime_lock(&relay_paths)?;

    // Prune expired principal-store records once at startup. A corrupt store is
    // fatal here: it would otherwise reject every Hello (which loads the store),
    // so surfacing it at startup fails fast rather than per connection.
    let pruned_principals =
        crate::relay::prune_principal_store(&roots.state_root).map_err(shared::map_relay_error)?;
    if pruned_principals > 0 {
        emit_inscription(
            "relay.identity.store_pruned",
            &json!({ "pruned_count": pruned_principals }),
        );
    }

    let mut outcomes = Vec::with_capacity(memberships.len());
    let mut hosted_bundles = Vec::<HostedBundle>::with_capacity(memberships.len());
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

    let listener = bind_relay_listener(&relay_paths)?;

    Ok(RelayHostPreparation::Serve(Box::new(RelayHostServePlan {
        summary,
        hosted_bundles,
        relay_paths,
        listener,
        runtime_lock,
    })))
}

/// Drives the single relay accept loop on the async runtime until shutdown,
/// then performs runtime cleanup.
#[allow(clippy::too_many_arguments)]
async fn serve_relay_host(
    roots: RuntimeRoots,
    summary: RelayHostStartupSummary,
    hosted_bundles: Vec<HostedBundle>,
    relay_paths: RelayRuntimePaths,
    listener: UnixListener,
    runtime_lock: RelayRuntimeLock,
    require_session_credentials: bool,
    watch_bundles: bool,
) -> Result<(), RuntimeError> {
    emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
    render_startup_summary(&summary);

    let stop_requested = Arc::new(AtomicBool::new(false));
    let max_connections = relay_max_connections();
    let connection_permits = Arc::new(Semaphore::new(max_connections));

    // Build the bundle catalog: map from bundle_name to per-bundle paths.
    // Connection workers consult this on Hello to route to the correct bundle.
    // The catalog is the live source of truth once the watcher can mutate it, so
    // the shutdown cleanup list is taken from it after the watcher is stopped
    // (below) rather than from this initial set.
    let catalog = BundleCatalog::from_paths(hosted_bundles.into_iter().map(|hosted| hosted.paths));

    // Remove any stale sentinel left by a crashed predecessor before we publish
    // ours. Otherwise a waiter could observe "stale sentinel + new socket
    // connectable" between socket bind (already completed in startup) and our
    // publish below, and falsely conclude the relay is serving.
    remove_relay_ready_sentinel(&relay_paths);

    let accept_handle = {
        let configuration_root = roots.configuration_root.clone();
        let state_root = roots.state_root.clone();
        let stop_requested = Arc::clone(&stop_requested);
        let connection_permits = Arc::clone(&connection_permits);
        let catalog = catalog.clone();
        // `require_session_credentials` is the relay-wide enforcement flag set by
        // the `--require-credentials` CLI flag (default: disabled).
        tokio::spawn(run_relay_accept_loop(
            configuration_root,
            state_root,
            listener,
            catalog,
            require_session_credentials,
            stop_requested,
            connection_permits,
            max_connections,
        ))
    };

    // Publish the ready sentinel only after the accept loop has been spawned.
    // The signal handler is already installed (in `run_relay_host`), so
    // `relay_socket_connectable && relay.ready exists` is a sound readiness
    // gate for callers waiting on relay startup.
    write_relay_ready_sentinel(&relay_paths)?;

    // Start watching the bundles configuration directory for runtime
    // add/remove/modify unless `--no-watch` was given. The guard is held for the
    // serving lifetime and dropped (stopping the watch) before cleanup tears the
    // hosted bundles down. A watcher that cannot be created (e.g. the platform
    // lacks filesystem notifications) is non-fatal: the relay keeps serving
    // without dynamic reconciliation.
    let bundle_watcher = if watch_bundles {
        match spawn_bundle_watcher(
            &roots.configuration_root,
            &roots.state_root,
            catalog.clone(),
        ) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                emit_inscription(
                    "relay.bundle.watch.unavailable",
                    &json!({ "cause": error.to_string() }),
                );
                None
            }
        }
    } else {
        None
    };

    let accept_outcome = supervise_accept_loop(&stop_requested, accept_handle).await;

    if shutdown_requested() {
        emit_inscription("relay.shutdown.signal", &json!({"signal": "termination"}));
    }
    stop_requested.store(true, Ordering::SeqCst);

    // Stop watching before cleanup unloads the bundles, so a reconcile cannot
    // race the teardown. Dropping the watcher joins its reconcile thread, so any
    // in-flight reload runs to completion before this returns.
    drop(bundle_watcher);

    // Snapshot the live catalog for cleanup now that the watcher is stopped: this
    // reflects bundles loaded or unloaded at runtime, so a runtime-added bundle's
    // tmux runtime is still torn down (and a runtime-removed one is not retried).
    let bundle_paths_for_cleanup = catalog.snapshot();

    // Cleanup (async-delivery drain, socket removal, tmux shutdown per bundle)
    // is blocking. The relay-level runtime lock is held until cleanup returns.
    let cleanup_relay_paths = relay_paths.clone();
    tokio::task::spawn_blocking(move || {
        cleanup_relay_host(cleanup_relay_paths, bundle_paths_for_cleanup)
    })
    .await
    .map_err(|source| supervisor_join_error("relay host cleanup", source))??;

    drop(runtime_lock);

    accept_outcome
}

/// Awaits the single accept loop until it completes or a shutdown signal
/// arrives. A clean accept-loop exit (graceful stop) is tolerated; a failure
/// propagates to the caller.
async fn supervise_accept_loop(
    stop_requested: &Arc<AtomicBool>,
    accept_handle: tokio::task::JoinHandle<Result<(), RuntimeError>>,
) -> Result<(), RuntimeError> {
    let mut accept_handle = accept_handle;
    loop {
        if shutdown_requested() || stop_requested.load(Ordering::SeqCst) {
            // Drive the loop to completion so the JoinHandle is consumed.
            return accept_loop_join_outcome(accept_handle.await);
        }
        tokio::select! {
            biased;
            joined = &mut accept_handle => {
                return accept_loop_join_outcome(joined);
            }
            () = tokio::time::sleep(Duration::from_millis(RELAY_SHUTDOWN_POLL_INTERVAL_MS)) => {}
        }
    }
}

fn accept_loop_join_outcome(
    result: Result<Result<(), RuntimeError>, tokio::task::JoinError>,
) -> Result<(), RuntimeError> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(join_error) => Err(RuntimeError::validation(
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

/// Performs runtime cleanup after the accept loop has stopped: drains async
/// delivery workers (on shutdown), removes the relay socket and sentinel, and
/// shuts down tmux per bundle.
fn cleanup_relay_host(
    relay_paths: RelayRuntimePaths,
    bundle_paths: Vec<BundleRuntimePaths>,
) -> Result<(), RuntimeError> {
    let async_workers_remaining = if shutdown_requested() {
        wait_for_async_delivery_shutdown(Duration::from_millis(1_500))
    } else {
        0
    };
    remove_relay_ready_sentinel(&relay_paths);
    shared::remove_relay_socket_file(&relay_paths.relay_socket)?;
    for paths in bundle_paths {
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

/// Writes the relay ready sentinel atomically (write-tmp + rename) so callers
/// that poll for its existence never see a partially-written file.
fn write_relay_ready_sentinel(paths: &RelayRuntimePaths) -> Result<(), RuntimeError> {
    let sentinel = &paths.relay_ready_sentinel;
    let temporary = sentinel.with_extension("ready.tmp");
    std::fs::write(&temporary, b"").map_err(|source| {
        RuntimeError::io(
            format!("write relay ready sentinel {}", temporary.display()),
            source,
        )
    })?;
    std::fs::rename(&temporary, sentinel).map_err(|source| {
        RuntimeError::io(
            format!("publish relay ready sentinel {}", sentinel.display()),
            source,
        )
    })
}

/// Best-effort sentinel removal during shutdown cleanup. A missing sentinel is
/// not an error; nor is a removal failure (the caller has already lost the
/// relay process and we should not block the rest of cleanup).
fn remove_relay_ready_sentinel(paths: &RelayRuntimePaths) {
    let _ = std::fs::remove_file(&paths.relay_ready_sentinel);
}

/// Accepts connections on the single relay socket and spawns a task per
/// connection, bounded by the shared connection semaphore. At the cap, new
/// connections receive an immediate overloaded response. Bundle routing is
/// deferred to the connection worker's Hello handling.
#[allow(clippy::too_many_arguments)]
async fn run_relay_accept_loop(
    configuration_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    listener: UnixListener,
    bundle_catalog: BundleCatalog,
    require_session_credentials: bool,
    stop_requested: Arc<AtomicBool>,
    connection_permits: Arc<Semaphore>,
    max_connections: usize,
) -> Result<(), RuntimeError> {
    listener
        .set_nonblocking(true)
        .map_err(|source| RuntimeError::io("set relay socket non-blocking".to_string(), source))?;
    let listener = TokioUnixListener::from_std(listener).map_err(|source| {
        RuntimeError::io(
            "register relay socket with async runtime".to_string(),
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
                                    state_root.clone(),
                                    bundle_catalog.clone(),
                                    require_session_credentials,
                                    Arc::clone(&metrics),
                                );
                            }
                            Err(TryAcquireError::NoPermits) => {
                                reject_overloaded_connection(stream, &metrics).await;
                            }
                            Err(TryAcquireError::Closed) => break Ok(()),
                        }
                        emit_connection_metrics(max_connections, &metrics);
                    }
                    Err(source) => {
                        break Err(RuntimeError::io(
                            "accept relay socket connection".to_string(),
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
#[allow(clippy::too_many_arguments)]
fn spawn_connection_worker(
    permit: OwnedSemaphorePermit,
    stream: TokioUnixStream,
    configuration_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    bundle_catalog: BundleCatalog,
    require_session_credentials: bool,
    metrics: Arc<RelayConnectionMetrics>,
) {
    metrics.active_connections.fetch_add(1, Ordering::SeqCst);
    let pre_hello_idle_timeout = relay_pre_hello_idle_timeout();
    tokio::spawn(async move {
        let result = serve_connection(
            stream,
            &configuration_root,
            &state_root,
            &bundle_catalog,
            require_session_credentials,
            pre_hello_idle_timeout,
        )
        .await;
        metrics.active_connections.fetch_sub(1, Ordering::SeqCst);
        drop(permit);
        if let Err(source) = result {
            emit_inscription(
                "relay.request_failed",
                &json!({
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
    mut stream: TokioUnixStream,
    metrics: &RelayConnectionMetrics,
) {
    metrics.rejected_connections.fetch_add(1, Ordering::SeqCst);
    emit_inscription(
        "relay.connection.rejected",
        &json!({
            "reason_code": "runtime_connection_limit_reached",
            "reason": "relay connection limit reached",
        }),
    );
    let response = crate::relay::RelayResponse::Error {
        error: crate::relay::RelayError {
            code: "runtime_connection_limit_reached".to_string(),
            message: "relay connection limit reached".to_string(),
            details: None,
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
fn emit_connection_metrics(max_connections: usize, metrics: &RelayConnectionMetrics) {
    emit_inscription(
        "relay.connection_pool.metrics",
        &json!({
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
) -> (RelayHostStartupBundle, Option<HostedBundle>) {
    let paths = match BundleRuntimePaths::resolve(&roots.state_root, bundle_name) {
        Ok(paths) => paths,
        Err(source) => return (failed_startup_bundle(bundle_name, source), None),
    };

    emit_inscription(
        "relay.startup",
        &json!({
            "bundle_name": paths.bundle_name,
            "tmux_socket": paths.tmux_socket,
            "configuration_root": roots.configuration_root,
            "state_root": roots.state_root,
            "inscriptions_root": roots.inscriptions_root,
        }),
    );
    if let Err(source) = ensure_bundle_runtime_directory(&paths) {
        return (failed_startup_bundle(bundle_name, source), None);
    }
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
    (startup_bundle, Some(HostedBundle { paths }))
}
