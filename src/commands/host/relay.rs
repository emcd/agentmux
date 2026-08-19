use std::{
    env,
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
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
        BundleCatalog, ConnectionDrainCoordinator, ConnectionServeContext, HostingIntent,
        PeerConnectionManager, configure_delivery, configured_undelivered_reporting,
        persist_startup_failures, report_undelivered_queue, serve_connection,
        shutdown_bundle_runtime, spawn_bundle_watcher, startup_bundle,
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
            BundleRuntimePaths, RelayRuntimePaths, RuntimeRootOverrides, RuntimeRoots,
            ensure_bundle_runtime_directory, ensure_relay_runtime_directory,
        },
        signals::{
            arm_shutdown_deadline, budget_within_shutdown, install_shutdown_signal_handlers,
            register_shutdown_grace, shutdown_requested,
        },
        starter::ensure_starter_configuration_layout,
    },
};

use crate::commands::{
    RelayHostArguments, RelayHostStartupBundle, RelayHostStartupSummary, RuntimeArguments, shared,
};

use super::summary::{
    build_startup_summary, degraded_startup_bundle, failed_autostart_bundle, failed_startup_bundle,
    failed_startup_bundle_from_relay_error, hosted_startup_bundle, render_startup_summary,
    skipped_startup_bundle, startup_summary_payload,
};

#[derive(Clone, Copy, Debug)]
enum RelayHostStartupMode {
    Autostart,
    ProcessOnly,
}

#[derive(Debug)]
struct HostedBundle {
    paths: BundleRuntimePaths,
    /// Initial hosting intent for the catalog entry: `Run` for an autostarted
    /// bundle, `Hold` for a process-only one (per-bundle `autostart = false` or
    /// relay-wide `--no-autostart`).
    hosting_intent: HostingIntent,
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
    relay_paths: RelayRuntimePaths,
    listener: UnixListener,
    runtime_lock: RelayRuntimeLock,
    // Resolved relay-wide controls (CLI > env > relay.toml > defaults), computed
    // once in the blocking startup phase and carried to the async serve phase so
    // it never re-reads or re-resolves relay.toml.
    watch_bundles: bool,
    // The relay-host flag that controls autostart vs process-only semantics for
    // every bundle the watcher later loads; carried through the plan so the
    // serve phase signature does not have to thread it as a loose argument.
    no_autostart: bool,
    // Pre-built per the resolved relay-wide controls and the outbound peer
    // connection manager; cloned (cheaply) per accepted connection. Carrying it
    // in the plan keeps the serve phase signature within the project's argument
    // budget without `#[allow(clippy::too_many_arguments)]`.
    serve_context: ConnectionServeContext,
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
// Grace period between observing a termination signal and forcing process
// exit. Caps all post-signal work (drain, cleanup, runtime teardown); graceful
// shutdown normally completes well under 2 seconds.
const RELAY_SHUTDOWN_WATCHDOG_GRACE_MS: u64 = 5_000;
// Bounded wait for connection workers to drain after the cooperative shutdown
// signal fires. Must leave headroom inside the watchdog grace for the rest of
// cleanup (async-delivery drain, tmux teardown); workers that miss the window
// are abandoned to runtime teardown and reported as timed out.
const RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS: u64 = 1_500;
// Headroom held back from the async-delivery drain for the work that follows it
// inside cleanup: sentinel and socket removal, then per-bundle tmux teardown,
// which spawns tmux clients and is the slowest of the three. A drain that spent
// the whole remaining grace would satisfy its own bound and leave the teardown
// to be cut off by the watchdog.
const RELAY_SHUTDOWN_TEARDOWN_RESERVE_MS: u64 = 1_000;

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
    spawn_shutdown_watchdog()?;

    // Startup (config load, tmux autostart, lock acquisition, socket binding) is
    // blocking, so it runs on a blocking task. The async serve phase then drives
    // the per-bundle accept loops on the runtime (tokio::net + tokio::spawn).
    // Resolved `watch_bundles` / `require_session_credentials` and the
    // `no_autostart` flag are computed from arguments/relay.toml during
    // startup and carried out on the serve plan so the serve phase does not
    // need to re-read any of them.
    let preparation = tokio::task::spawn_blocking(move || {
        let roots = resolve_runtime_roots(arguments.runtime)?;
        let preparation = prepare_relay_host(
            &roots,
            arguments.no_autostart,
            arguments.watch_bundles,
            arguments.require_session_credentials,
        )?;
        Ok::<_, RuntimeError>(preparation)
    })
    .await
    .map_err(|source| supervisor_join_error("relay host startup", source))??;

    match preparation {
        RelayHostPreparation::NoHostedBundles => Ok(()),
        RelayHostPreparation::Serve(plan) => serve_relay_host(plan).await,
    }
}

/// Spawns the runtime-independent shutdown watchdog: a plain OS thread that
/// polls the shutdown flag and forces process exit a bounded grace period
/// after a termination signal is observed.
///
/// This is the hard backstop for issues/relay/26: request dispatch runs
/// blocking work inline on tokio worker threads, so once every worker parks,
/// the timer-driven shutdown polls in `supervise_accept_loop` and the accept
/// loop can never observe SIGTERM and the process hangs until SIGKILL. The
/// watchdog shares nothing with the runtime (OS thread, `thread::sleep`,
/// `process::exit`), so it wins whenever graceful shutdown stalls.
///
/// If the relay wedges again with this backstop deployed, capture thread state
/// before the grace period expires (or before resorting to SIGKILL):
/// `eu-stack -p <relay pid>` or
/// `gdb -p <relay pid> -ex "thread apply all bt" -ex quit`.
fn spawn_shutdown_watchdog() -> Result<(), RuntimeError> {
    // Registered before the thread exists, and therefore before any signal can
    // be observed. The watchdog cannot arm the deadline until it notices the
    // flag a poll interval later, and cleanup can begin inside that window —
    // registering the grace up front is what lets whoever needs a budget first
    // establish the deadline instead of concluding this process has none.
    register_shutdown_grace(Duration::from_millis(RELAY_SHUTDOWN_WATCHDOG_GRACE_MS));
    thread::Builder::new()
        .name("shutdown-watchdog".to_string())
        .spawn(|| {
            while !shutdown_requested() {
                thread::sleep(Duration::from_millis(RELAY_SHUTDOWN_POLL_INTERVAL_MS));
            }
            // Publish the deadline before announcing it, and before sleeping the
            // grace. This is the *later* of the two arming paths: cleanup can
            // begin inside the poll interval above, and a step needing a budget
            // there arms from the registered grace rather than waiting for this.
            // First-arming-wins, so that earlier deadline is the one that
            // stands — earlier than this observation, and therefore earlier than
            // the forced exit it has to precede.
            arm_shutdown_deadline(Duration::from_millis(RELAY_SHUTDOWN_WATCHDOG_GRACE_MS));
            emit_inscription(
                "relay.shutdown.watchdog.armed",
                &json!({ "grace_ms": RELAY_SHUTDOWN_WATCHDOG_GRACE_MS }),
            );
            eprintln!(
                "relay shutdown watchdog armed: forcing exit in \
                 {RELAY_SHUTDOWN_WATCHDOG_GRACE_MS} ms unless graceful shutdown completes"
            );
            thread::sleep(Duration::from_millis(RELAY_SHUTDOWN_WATCHDOG_GRACE_MS));
            emit_inscription(
                "relay.shutdown.watchdog.forced_exit",
                &json!({ "grace_ms": RELAY_SHUTDOWN_WATCHDOG_GRACE_MS }),
            );
            eprintln!(
                "relay shutdown watchdog: graceful shutdown did not complete within \
                 {RELAY_SHUTDOWN_WATCHDOG_GRACE_MS} ms; forcing exit"
            );
            std::process::exit(0);
        })
        .map_err(|source| RuntimeError::io("spawn relay shutdown watchdog thread", source))?;
    Ok(())
}

fn resolve_runtime_roots(runtime: RuntimeArguments) -> Result<RuntimeRoots, RuntimeError> {
    // The same resolver every other surface uses. A relay that answered this
    // question differently from its clients would bind a socket none of them
    // look for.
    let overrides = RuntimeRootOverrides {
        configuration_layers: runtime.configuration_layers,
        state_root: runtime.state_root,
        inscriptions_root: runtime.inscriptions_root,
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    ensure_starter_configuration_layout(&roots)?;
    Ok(roots)
}

/// Runs the synchronous startup phase: acquire the relay-level lock, bind
/// the single relay socket, host each bundle, and decide whether there is
/// anything to serve.
fn prepare_relay_host(
    roots: &RuntimeRoots,
    no_autostart: bool,
    cli_watch_bundles: Option<bool>,
    cli_require_session_credentials: Option<bool>,
) -> Result<RelayHostPreparation, RuntimeError> {
    // Resolve relay-wide controls before any bundle work so a malformed
    // `relay.toml` (bad type, unknown field, nested `[relay]` table, invalid peer
    // entry, or invalid environment override) fails startup up front, regardless
    // of how many bundles are configured.
    let relay_configuration = crate::relay::load_relay_runtime_configuration(
        &roots.configuration_roots,
        cli_watch_bundles,
        cli_require_session_credentials,
    )
    .map_err(shared::map_relay_error)?;
    // Published before anything can be admitted: the listener is not bound yet,
    // so no request boundary has run and no entry can have reserved quota against
    // the defaults.
    configure_delivery(relay_configuration.delivery);

    let memberships = load_bundle_group_memberships(&roots.configuration_roots)
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

    // Register `users.toml`-declared relay-wide principals as static (offline)
    // registry entries so look/raww resolve their capabilities from the unified
    // registry and a declared-but-disconnected principal is a known target.
    crate::relay::register_configured_relay_wide_principals(&roots.configuration_roots)
        .map_err(shared::map_relay_error)?;

    // A restart-first operator diagnoses from the journal and inscriptions, so
    // every failed bundle must leave a per-bundle reason on both before any
    // aggregate error can exit the process.
    for outcome in &outcomes {
        if outcome.outcome != "failed" {
            continue;
        }
        let reason_code = outcome.reason_code.as_deref().unwrap_or("unknown");
        let reason = outcome.reason.as_deref().unwrap_or("unknown");
        match &outcome.details {
            Some(details) => eprintln!(
                "bundle '{}' failed to start ({reason_code}): {reason}; details: {details}",
                outcome.bundle_name
            ),
            None => eprintln!(
                "bundle '{}' failed to start ({reason_code}): {reason}",
                outcome.bundle_name
            ),
        }
        emit_inscription(
            "relay.bundle.startup_failed",
            &json!({
                "bundle_name": outcome.bundle_name,
                "reason_code": outcome.reason_code,
                "reason": outcome.reason,
                "details": outcome.details,
            }),
        );
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
            emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
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

    // Build the outbound peer connection manager once from the resolved peers,
    // before that field is consumed below. Each peer carries its own presented
    // identity (`connect-as`); the manager is empty (and never dials) on a relay
    // with no configured peers.
    let peer_connection_manager = Arc::new(PeerConnectionManager::from_configuration(
        &roots.state_root,
        &relay_configuration.peers,
    ));

    // Build the bundle catalog and pre-build the shared serving context so the
    // serve phase constructs `ConnectionServeContext` exactly once and clones
    // (cheaply, via the inner `Arc`s) per accepted connection. Carrying the
    // already-built context in the plan keeps the serve-phase signature within
    // the project's argument budget without resorting to a clippy suppression.
    let catalog = BundleCatalog::from_entries(
        hosted_bundles
            .into_iter()
            .map(|hosted| (hosted.paths.clone(), hosted.hosting_intent)),
    );
    let pre_hello_idle_timeout = relay_pre_hello_idle_timeout();
    // Configured outbound peer aliases, read from the normalized `[[peers]]`
    // entries so `list.relays` enumerates the routing table without dialing.
    let relay_aliases = relay_configuration
        .peers
        .iter()
        .map(|peer| peer.alias.clone())
        .collect::<Vec<_>>();
    let serve_context = ConnectionServeContext::new(
        roots.configuration_roots.clone(),
        roots.state_root.clone(),
        catalog,
        peer_connection_manager,
        relay_aliases,
        relay_configuration.require_session_credentials,
        pre_hello_idle_timeout,
    );

    Ok(RelayHostPreparation::Serve(Box::new(RelayHostServePlan {
        summary,
        relay_paths,
        listener,
        runtime_lock,
        watch_bundles: relay_configuration.watch_bundles,
        no_autostart,
        serve_context,
    })))
}

/// Drives the single relay accept loop on the async runtime until shutdown,
/// then performs runtime cleanup.
async fn serve_relay_host(plan: Box<RelayHostServePlan>) -> Result<(), RuntimeError> {
    let RelayHostServePlan {
        summary,
        relay_paths,
        listener,
        runtime_lock,
        watch_bundles,
        no_autostart,
        serve_context,
    } = *plan;
    emit_inscription("relay.startup.summary", &startup_summary_payload(&summary));
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
            }
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

    let accept_outcome = supervise_accept_loop(&stop_requested, accept_handle).await;

    if shutdown_requested() {
        emit_inscription("relay.shutdown.signal", &json!({"signal": "termination"}));
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
async fn run_relay_accept_loop(
    listener: UnixListener,
    serve_context: ConnectionServeContext,
    stop_requested: Arc<AtomicBool>,
    connection_permits: Arc<Semaphore>,
    max_connections: usize,
    drain_coordinator: Arc<ConnectionDrainCoordinator>,
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
    // The undelivered-queue report rides this loop rather than a task of its own:
    // it is exactly relay-lifetime scoped, needs no shutdown coordination, and
    // does no work worth a thread. The first tick fires immediately, so skip it —
    // a relay that has just started has nothing queued to report.
    let undelivered_reporting = configured_undelivered_reporting();
    let mut undelivered_report = tokio::time::interval(undelivered_reporting.interval);
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
fn spawn_connection_worker(
    permit: OwnedSemaphorePermit,
    stream: TokioUnixStream,
    serve_context: ConnectionServeContext,
    metrics: Arc<RelayConnectionMetrics>,
    drain_coordinator: &Arc<ConnectionDrainCoordinator>,
) {
    metrics.active_connections.fetch_add(1, Ordering::SeqCst);
    let worker_slot = drain_coordinator.register_worker();
    tokio::spawn(async move {
        let result = serve_connection(stream, &serve_context, worker_slot).await;
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
            "configuration_layers": roots.configuration_roots.layers(),
            "state_root": roots.state_root,
            "inscriptions_root": roots.inscriptions_root,
        }),
    );
    if let Err(source) = ensure_bundle_runtime_directory(&paths) {
        return (failed_startup_bundle(bundle_name, source), None);
    }
    // Process-only hosting skips `startup_bundle`, so register the configured
    // members as static registry shells here; otherwise the unified registry would
    // omit every configured principal until its first Hello, and look/raww/list
    // would treat a declared-but-offline member as an unknown target. Autostart
    // performs this registration inside `startup_bundle`.
    if let RelayHostStartupMode::ProcessOnly = startup_mode
        && let Err(source) =
            crate::relay::register_configured_bundle(&roots.configuration_roots, &paths.bundle_name)
    {
        return (
            failed_startup_bundle_from_relay_error(bundle_name, source),
            None,
        );
    }
    let mut startup_report = None;
    if let RelayHostStartupMode::Autostart = startup_mode {
        let report = match startup_bundle(&roots.configuration_roots, &paths) {
            Ok(report) => report,
            Err(source) => {
                return (
                    failed_startup_bundle_from_relay_error(bundle_name, source),
                    None,
                );
            }
        };
        let mut report = report;
        match persist_startup_failures(
            bundle_name,
            &paths.runtime_directory,
            &report.failed_startups,
        ) {
            Ok(persisted) => report.failed_startups = persisted,
            Err(cause) => {
                return (
                    RelayHostStartupBundle {
                        bundle_name: bundle_name.to_string(),
                        outcome: "failed".to_string(),
                        reason_code: Some("runtime_startup_failed".to_string()),
                        reason: Some(format!(
                            "failed to persist startup failure history: {cause}"
                        )),
                        details: None,
                    },
                    None,
                );
            }
        }
        startup_report = Some(report);
    }
    let startup_bundle = match startup_mode {
        RelayHostStartupMode::Autostart => {
            let report = startup_report.expect("autostart report must be present");
            match (
                report.ready_session_count > 0,
                report.failed_startups.is_empty(),
            ) {
                (true, true) => hosted_startup_bundle(bundle_name),
                (true, false) => degraded_startup_bundle(bundle_name, &report.failed_startups),
                (false, _) => failed_autostart_bundle(bundle_name, &report.failed_startups),
            }
        }
        RelayHostStartupMode::ProcessOnly => skipped_startup_bundle(
            bundle_name,
            "process_only",
            "relay started without bundle autostart".to_string(),
        ),
    };
    // The startup mode already encodes the effective-autostart rule
    // (`no_autostart || !membership.autostart` => process-only), so the catalog's
    // initial hosting intent falls straight out of it.
    let hosting_intent = match startup_mode {
        RelayHostStartupMode::Autostart => HostingIntent::Run,
        RelayHostStartupMode::ProcessOnly => HostingIntent::Hold,
    };
    (
        startup_bundle,
        Some(HostedBundle {
            paths,
            hosting_intent,
        }),
    )
}
