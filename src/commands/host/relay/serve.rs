use std::{
    env,
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
    relay::{
        ConnectionDrainCoordinator, ConnectionServeContext,
        configured_undelivered_reporting, report_undelivered_queue,
        serve_connection, shutdown_bundle_runtime, spawn_bundle_watcher,
        wait_for_async_delivery_shutdown,
    },
    runtime::{
        error::RuntimeError,
        inscriptions::emit_inscription,
        paths::{BundleRuntimePaths, RelayRuntimePaths},
        signals::{budget_within_shutdown, shutdown_requested},
    },
};

use crate::commands::shared;

use super::super::summary::{render_startup_summary, startup_summary_payload};
use super::{
    RELAY_MAX_CONNECTIONS, RELAY_PRE_HELLO_IDLE_TIMEOUT_MS,
    RELAY_SHUTDOWN_POLL_INTERVAL_MS, RELAY_SHUTDOWN_TEARDOWN_RESERVE_MS,
    RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS, RelayConnectionMetrics,
    RelayHostServePlan,
};

/// Drives the single relay accept loop on the async runtime until shutdown,
/// then performs runtime cleanup.
pub(super) async fn serve_relay_host(
    plan: Box<RelayHostServePlan>,
) -> Result<(), RuntimeError> {
    let RelayHostServePlan {
        summary,
        relay_paths,
        listener,
        runtime_lock,
        watch_bundles,
        no_autostart,
        serve_context,
    } = *plan;
    emit_inscription(
        "relay.startup.summary",
        &startup_summary_payload(&summary),
    );
    render_startup_summary(&summary);

    let stop_requested = Arc::new(AtomicBool::new(false));
    let max_connections = relay_max_connections();
    let connection_permits = Arc::new(Semaphore::new(max_connections));
    let drain_coordinator = ConnectionDrainCoordinator::new();

    // Remove any stale sentinel left by a crashed predecessor before we publish
    // ours. Otherwise a waiter could observe "stale sentinel + new socket
    // connectable" between socket bind (already completed in startup) and our
    // publish below, and falsely conclude the relay is serving.
    remove_relay_ready_sentinel(&relay_paths);

    let accept_handle = {
        let stop_requested = Arc::clone(&stop_requested);
        let connection_permits = Arc::clone(&connection_permits);
        let drain_coordinator = Arc::clone(&drain_coordinator);
        // The accept loop owns one cheap clone of the shared serving context;
        // every per-connection clone happens *inside* the loop at
        // `spawn_connection_worker`. The original `serve_context` remains
        // available here for the watcher spawn and the cleanup snapshot.
        let accept_context = serve_context.clone();
        tokio::spawn(run_relay_accept_loop(
            listener,
            accept_context,
            stop_requested,
            connection_permits,
            max_connections,
            drain_coordinator,
        ))
    };

    // Start watching the bundles configuration directory for runtime
    // add/remove/modify unless resolved `watch-bundles` is false (CLI override,
    // environment override, or relay.toml). The guard is held for the
    // serving lifetime and dropped (stopping the watch) before cleanup tears the
    // hosted bundles down. A watcher that cannot be created (e.g. the platform
    // lacks filesystem notifications) is non-fatal: the relay keeps serving
    // without dynamic reconciliation.
    //
    // Spawned before the ready sentinel is published: `spawn_bundle_watcher`
    // returns with the filesystem watch armed and the content fingerprints
    // seeded, so a bundle-file mutation made after readiness is observable
    // as a change. Publishing readiness first would open a gap where an edit
    // is silently absorbed into the seeded fingerprints (never reconciled)
    // and a removal produces no event at all.
    let bundle_watcher = if watch_bundles {
        match spawn_bundle_watcher(
            serve_context.configuration_roots(),
            serve_context.state_root(),
            serve_context.bundle_catalog().clone(),
            no_autostart,
        ) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                emit_inscription(
                    "relay.bundle.watch.unavailable",
                    &json!({ "cause": error.to_string() }),
                );
                None
            },
        }
    } else {
        None
    };

    // Publish the ready sentinel only after the accept loop has been spawned
    // and the bundle watcher (when enabled) is armed. The signal handler is
    // already installed (in `run_relay_host`), so `relay_socket_connectable &&
    // relay.ready exists` is a sound readiness gate for callers waiting on
    // relay startup.
    write_relay_ready_sentinel(&relay_paths)?;

    let accept_outcome =
        supervise_accept_loop(&stop_requested, accept_handle).await;

    if shutdown_requested() {
        emit_inscription(
            "relay.shutdown.signal",
            &json!({"signal": "termination"}),
        );
    }
    stop_requested.store(true, Ordering::SeqCst);

    // Cooperative connection-worker drain: signal every worker to wrap up, then
    // wait a bounded window for them to exit. A worker parked on a stream read
    // exits immediately; one mid-request finishes its in-flight dispatch first.
    // Workers that miss the window are abandoned to runtime teardown (and the
    // shutdown watchdog), so this wait can never stall shutdown indefinitely.
    drain_coordinator.signal_shutdown();
    let drain_report = drain_coordinator
        .wait_for_drain(Duration::from_millis(
            RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS,
        ))
        .await;
    emit_inscription(
        "relay.shutdown.worker_drain",
        &json!({
            "drained_worker_count": drain_report.drained_worker_count,
            "remaining_worker_count": drain_report.remaining_worker_count,
            "remaining_serving_count": drain_report.remaining_serving_count,
            "timed_out": drain_report.timed_out,
            "drain_timeout_ms": RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS,
        }),
    );

    // Stop watching before cleanup unloads the bundles, so a reconcile cannot
    // race the teardown. Dropping the watcher joins its reconcile thread, so any
    // in-flight reload runs to completion before this returns.
    drop(bundle_watcher);

    // Snapshot the live catalog for cleanup now that the watcher is stopped: this
    // reflects bundles loaded or unloaded at runtime, so a runtime-added bundle's
    // tmux runtime is still torn down (and a runtime-removed one is not retried).
    let bundle_paths_for_cleanup = serve_context.bundle_catalog().snapshot();

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
pub(super) async fn supervise_accept_loop(
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

pub(super) fn accept_loop_join_outcome(
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
pub(super) fn supervisor_join_error(
    context: &str,
    source: tokio::task::JoinError,
) -> RuntimeError {
    RuntimeError::validation(
        "internal_unexpected_failure",
        format!("{context} task failed to join: {source}"),
    )
}

/// Performs runtime cleanup after the accept loop has stopped: drains async
/// delivery workers (on shutdown), removes the relay socket and sentinel, and
/// shuts down tmux per bundle.
pub(super) fn cleanup_relay_host(
    relay_paths: RelayRuntimePaths,
    bundle_paths: Vec<BundleRuntimePaths>,
) -> Result<(), RuntimeError> {
    let async_workers_remaining = if shutdown_requested() {
        // Fitted to the watchdog grace rather than taken outright. The configured
        // bound is the most this step may take; the deadline is what it must fit
        // inside, and the reserve is for the socket removal and per-bundle tmux
        // teardown below, which still have to run after this returns.
        wait_for_async_delivery_shutdown(budget_within_shutdown(
            Duration::from_millis(RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS),
            Duration::from_millis(RELAY_SHUTDOWN_TEARDOWN_RESERVE_MS),
        ))
    } else {
        0
    };
    remove_relay_ready_sentinel(&relay_paths);
    shared::remove_relay_socket_file(&relay_paths.relay_socket)?;
    for paths in bundle_paths {
        let shutdown = shutdown_bundle_runtime(
            paths.bundle_name.as_str(),
            &paths.runtime_directory,
            &paths.tmux_socket,
        )
        .map_err(shared::map_reconcile_error)?;
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
pub(super) fn write_relay_ready_sentinel(
    paths: &RelayRuntimePaths,
) -> Result<(), RuntimeError> {
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
pub(super) fn remove_relay_ready_sentinel(paths: &RelayRuntimePaths) {
    let _ = std::fs::remove_file(&paths.relay_ready_sentinel);
}

/// Accepts connections on the single relay socket and spawns a task per
/// connection, bounded by the shared connection semaphore. At the cap, new
/// connections receive an immediate overloaded response. Bundle routing is
/// deferred to the connection worker's Hello handling.
pub(super) async fn run_relay_accept_loop(
    listener: UnixListener,
    serve_context: ConnectionServeContext,
    stop_requested: Arc<AtomicBool>,
    connection_permits: Arc<Semaphore>,
    max_connections: usize,
    drain_coordinator: Arc<ConnectionDrainCoordinator>,
) -> Result<(), RuntimeError> {
    listener.set_nonblocking(true).map_err(|source| {
        RuntimeError::io("set relay socket non-blocking".to_string(), source)
    })?;
    let listener =
        TokioUnixListener::from_std(listener).map_err(|source| {
            RuntimeError::io(
                "register relay socket with async runtime".to_string(),
                source,
            )
        })?;
    let metrics = Arc::new(RelayConnectionMetrics::new());
    // The undelivered-queue report rides this loop rather than a task of its own:
    // it is exactly relay-lifetime scoped, needs no shutdown coordination, and
    // does no work worth a thread. The first tick fires immediately, so skip it —
    // a relay that has just started has nothing queued to report.
    let undelivered_reporting = configured_undelivered_reporting();
    let mut undelivered_report =
        tokio::time::interval(undelivered_reporting.interval);
    undelivered_report.tick().await;

    loop {
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
                                    serve_context.clone(),
                                    Arc::clone(&metrics),
                                    &drain_coordinator,
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
            _ = undelivered_report.tick() => {
                report_undelivered_queue(undelivered_reporting);
            }
            () = tokio::time::sleep(Duration::from_millis(RELAY_SHUTDOWN_POLL_INTERVAL_MS)) => {}
        }
    }
}

/// Spawns a tokio task to serve one accepted connection asynchronously. The
/// task is not joined: shutdown signals it cooperatively through its drain
/// coordinator slot (registered before the spawn, so a racing shutdown signal
/// still counts the worker), and any worker that misses the bounded drain
/// window is cancelled when the runtime is dropped, which triggers the
/// connection's drop-guard unregister path. Per-connection writes run on a
/// separate writer task spawned inside `serve_connection`, so no blocking call
/// ever ties up a runtime worker.
pub(super) fn spawn_connection_worker(
    permit: OwnedSemaphorePermit,
    stream: TokioUnixStream,
    serve_context: ConnectionServeContext,
    metrics: Arc<RelayConnectionMetrics>,
    drain_coordinator: &Arc<ConnectionDrainCoordinator>,
) {
    metrics.active_connections.fetch_add(1, Ordering::SeqCst);
    let worker_slot = drain_coordinator.register_worker();
    tokio::spawn(async move {
        let result =
            serve_connection(stream, &serve_context, worker_slot).await;
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

pub(super) fn relay_max_connections() -> usize {
    parse_env_positive_usize("AGENTMUX_RELAY_MAX_CONNECTIONS")
        .unwrap_or(RELAY_MAX_CONNECTIONS)
}

pub(super) fn relay_pre_hello_idle_timeout() -> Duration {
    let timeout_ms =
        parse_env_positive_usize("AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS")
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(RELAY_PRE_HELLO_IDLE_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

pub(super) fn parse_env_positive_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub(super) async fn reject_overloaded_connection(
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
pub(super) fn emit_connection_metrics(
    max_connections: usize,
    metrics: &RelayConnectionMetrics,
) {
    emit_inscription(
        "relay.connection_pool.metrics",
        &json!({
            "max_connections": max_connections,
            "active_connections": metrics.active_connections.load(Ordering::SeqCst),
            "rejected_connections": metrics.rejected_connections.load(Ordering::SeqCst),
        }),
    );
}
