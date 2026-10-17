use std::{sync::Arc, thread, time::Duration};

use serde_json::json;

use crate::{
    configuration::load_bundle_group_memberships,
    relay::{
        BundleCatalog, ConnectionServeContext, HostingIntent,
        PeerConnectionManager, configure_delivery, persist_startup_failures,
        startup_bundle,
    },
    runtime::{
        bootstrap::{
            acquire_relay_runtime_lock, bind_relay_listener,
            relay_runtime_lock_is_held,
        },
        error::RuntimeError,
        inscriptions::{
            configure_process_inscriptions, emit_inscription,
            relay_inscriptions_path,
        },
        paths::{
            BundleRuntimePaths, RelayRuntimePaths, RuntimeRootOverrides,
            RuntimeRoots, ensure_bundle_runtime_directory,
            ensure_relay_runtime_directory,
        },
        signals::{
            arm_shutdown_deadline, register_shutdown_grace, shutdown_requested,
        },
        starter::ensure_starter_configuration_layout,
    },
};

use crate::commands::{RelayHostStartupBundle, RuntimeArguments, shared};

use super::super::summary::{
    build_startup_summary, degraded_startup_bundle, failed_autostart_bundle,
    failed_startup_bundle, failed_startup_bundle_from_relay_error,
    hosted_startup_bundle, render_startup_summary, skipped_startup_bundle,
    startup_summary_payload,
};
use super::serve::relay_pre_hello_idle_timeout;
use super::{
    HostedBundle, RELAY_SHUTDOWN_POLL_INTERVAL_MS,
    RELAY_SHUTDOWN_WATCHDOG_GRACE_MS, RelayHostPreparation,
    RelayHostServePlan, RelayHostStartupMode,
};

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
pub(super) fn spawn_shutdown_watchdog() -> Result<(), RuntimeError> {
    // Registered before the thread exists, and therefore before any signal can
    // be observed. The watchdog cannot arm the deadline until it notices the
    // flag a poll interval later, and cleanup can begin inside that window —
    // registering the grace up front is what lets whoever needs a budget first
    // establish the deadline instead of concluding this process has none.
    register_shutdown_grace(Duration::from_millis(
        RELAY_SHUTDOWN_WATCHDOG_GRACE_MS,
    ));
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

pub(super) fn resolve_runtime_roots(
    runtime: RuntimeArguments,
) -> Result<RuntimeRoots, RuntimeError> {
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
pub(super) fn prepare_relay_host(
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
    // Spans both roots, so it cannot live inside the file loader: peers come
    // from the configuration roots, the principal store from the state root. A
    // peer whose alias names no identity this relay issued would authenticate
    // inbound under a name no route selects, so the reply path is broken before
    // the first delivery — reject at startup rather than at that delivery.
    crate::relay::validate_peer_aliases(
        &relay_configuration.peers,
        &roots.state_root,
    )
    .map_err(shared::map_relay_error)?;
    // Published before anything can be admitted: the listener is not bound yet,
    // so no request boundary has run and no entry can have reserved quota against
    // the defaults.
    configure_delivery(relay_configuration.delivery);

    let memberships =
        load_bundle_group_memberships(&roots.configuration_roots)
            .map_err(shared::map_bundle_load_error)?;
    configure_process_inscriptions(&relay_inscriptions_path(
        &roots.inscriptions_root,
    ))?;

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
        emit_inscription(
            "relay.startup.summary",
            &startup_summary_payload(&summary),
        );
        render_startup_summary(&summary);
        return Ok(RelayHostPreparation::NoHostedBundles);
    }
    let runtime_lock = acquire_relay_runtime_lock(&relay_paths)?;

    // Prune expired principal-store records once at startup. A corrupt store is
    // fatal here: it would otherwise reject every Hello (which loads the store),
    // so surfacing it at startup fails fast rather than per connection.
    let pruned_principals =
        crate::relay::prune_principal_store(&roots.state_root)
            .map_err(shared::map_relay_error)?;
    if pruned_principals > 0 {
        emit_inscription(
            "relay.identity.store_pruned",
            &json!({ "pruned_count": pruned_principals }),
        );
    }

    let mut outcomes = Vec::with_capacity(memberships.len());
    let mut hosted_bundles =
        Vec::<HostedBundle>::with_capacity(memberships.len());
    for membership in memberships {
        let startup_mode = if no_autostart || !membership.autostart {
            RelayHostStartupMode::ProcessOnly
        } else {
            RelayHostStartupMode::Autostart
        };
        let (mut outcome, hosted_bundle) = host_selected_bundle(
            roots,
            membership.bundle_name.as_str(),
            startup_mode,
        );
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
    crate::relay::register_configured_relay_wide_principals(
        &roots.configuration_roots,
    )
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
            emit_inscription(
                "relay.startup.summary",
                &startup_summary_payload(&summary),
            );
            render_startup_summary(&summary);
            return Ok(RelayHostPreparation::NoHostedBundles);
        }
        if summary.failed_bundle_count > 0 {
            emit_inscription(
                "relay.startup.summary",
                &startup_summary_payload(&summary),
            );
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
    let peer_connection_manager =
        Arc::new(PeerConnectionManager::from_configuration(
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

pub(super) fn host_selected_bundle(
    roots: &RuntimeRoots,
    bundle_name: &str,
    startup_mode: RelayHostStartupMode,
) -> (RelayHostStartupBundle, Option<HostedBundle>) {
    let paths =
        match BundleRuntimePaths::resolve(&roots.state_root, bundle_name) {
            Ok(paths) => paths,
            Err(source) => {
                return (failed_startup_bundle(bundle_name, source), None);
            },
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
        && let Err(source) = crate::relay::register_configured_bundle(
            &roots.configuration_roots,
            &paths.bundle_name,
        )
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
                    failed_startup_bundle_from_relay_error(
                        bundle_name,
                        source,
                    ),
                    None,
                );
            },
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
                        reason_code: Some(
                            "runtime_startup_failed".to_string(),
                        ),
                        reason: Some(format!(
                            "failed to persist startup failure history: {cause}"
                        )),
                        details: None,
                    },
                    None,
                );
            },
        }
        startup_report = Some(report);
    }
    let startup_bundle = match startup_mode {
        RelayHostStartupMode::Autostart => {
            let report =
                startup_report.expect("autostart report must be present");
            match (
                report.ready_session_count > 0,
                report.failed_startups.is_empty(),
            ) {
                (true, true) => hosted_startup_bundle(bundle_name),
                (true, false) => degraded_startup_bundle(
                    bundle_name,
                    &report.failed_startups,
                ),
                (false, _) => failed_autostart_bundle(
                    bundle_name,
                    &report.failed_startups,
                ),
            }
        },
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
