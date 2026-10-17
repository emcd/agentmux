mod prepare;
mod serve;

use std::{os::unix::net::UnixListener, sync::atomic::AtomicUsize};

use crate::{
    relay::{ConnectionServeContext, HostingIntent},
    runtime::{
        bootstrap::RelayRuntimeLock,
        error::RuntimeError,
        paths::{BundleRuntimePaths, RelayRuntimePaths},
        signals::install_shutdown_signal_handlers,
    },
};

use crate::commands::{RelayHostArguments, RelayHostStartupSummary};

pub(super) enum RelayHostStartupMode {
    Autostart,
    ProcessOnly,
}

#[derive(Debug)]
pub(super) struct HostedBundle {
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
pub(super) enum RelayHostPreparation {
    NoHostedBundles,
    Serve(Box<RelayHostServePlan>),
}

#[derive(Debug)]
pub(super) struct RelayHostServePlan {
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
pub(super) struct RelayConnectionMetrics {
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
pub(super) const RELAY_MAX_CONNECTIONS: usize = 512;
pub(super) const RELAY_PRE_HELLO_IDLE_TIMEOUT_MS: u64 = 2_000;
pub(super) const RELAY_SHUTDOWN_POLL_INTERVAL_MS: u64 = 100;
// Grace period between observing a termination signal and forcing process
// exit. Caps all post-signal work (drain, cleanup, runtime teardown); graceful
// shutdown normally completes well under 2 seconds.
pub(super) const RELAY_SHUTDOWN_WATCHDOG_GRACE_MS: u64 = 5_000;
// Bounded wait for connection workers to drain after the cooperative shutdown
// signal fires. Must leave headroom inside the watchdog grace for the rest of
// cleanup (async-delivery drain, tmux teardown); workers that miss the window
// are abandoned to runtime teardown and reported as timed out.
pub(super) const RELAY_SHUTDOWN_WORKER_DRAIN_TIMEOUT_MS: u64 = 1_500;
// Headroom held back from the async-delivery drain for the work that follows it
// inside cleanup: sentinel and socket removal, then per-bundle tmux teardown,
// which spawns tmux clients and is the slowest of the three. A drain that spent
// the whole remaining grace would satisfy its own bound and leave the teardown
// to be cut off by the watchdog.
pub(super) const RELAY_SHUTDOWN_TEARDOWN_RESERVE_MS: u64 = 1_000;

pub(super) async fn run_relay_host(
    arguments: RelayHostArguments,
) -> Result<(), RuntimeError> {
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
    prepare::spawn_shutdown_watchdog()?;

    // Startup (config load, tmux autostart, lock acquisition, socket binding) is
    // blocking, so it runs on a blocking task. The async serve phase then drives
    // the per-bundle accept loops on the runtime (tokio::net + tokio::spawn).
    // Resolved `watch_bundles` / `require_session_credentials` and the
    // `no_autostart` flag are computed from arguments/relay.toml during
    // startup and carried out on the serve plan so the serve phase does not
    // need to re-read any of them.
    let preparation = tokio::task::spawn_blocking(move || {
        let roots = prepare::resolve_runtime_roots(arguments.runtime)?;
        let preparation = prepare::prepare_relay_host(
            &roots,
            arguments.no_autostart,
            arguments.watch_bundles,
            arguments.require_session_credentials,
        )?;
        Ok::<_, RuntimeError>(preparation)
    })
    .await
    .map_err(|source| {
        serve::supervisor_join_error("relay host startup", source)
    })??;

    match preparation {
        RelayHostPreparation::NoHostedBundles => Ok(()),
        RelayHostPreparation::Serve(plan) => {
            serve::serve_relay_host(plan).await
        },
    }
}
