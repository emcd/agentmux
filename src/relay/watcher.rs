//! Runtime bundle file watcher.
//!
//! Watches every configuration layer and reconciles the loaded bundle
//! set against the effective bundle union whenever a debounced change arrives.
//! New bundle files are loaded and started; removed files unload their bundle
//! (evicting active sessions with `runtime_bundle_unloaded`); modified files are
//! treated as a full teardown + reload (evicting active sessions with
//! `runtime_bundle_reloaded`). Smart reload (disconnecting only sessions whose
//! definitions changed) is deferred as a follow-on; this change tears down every
//! session in a modified bundle.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use notify_debouncer_full::{
    DebounceEventResult, Debouncer, RecommendedCache, new_debouncer,
    notify::{EventKind, RecommendedWatcher, RecursiveMode},
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::configuration::{
    ConfigurationRoots, bundle_configuration_path, effective_bundle_definitions,
    load_bundle_configuration,
};
use crate::runtime::error::RuntimeError;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};

use super::catalog::{BundleCatalog, HostingIntent};
use super::lifecycle::{
    register_configured_bundle_principals, shutdown_bundle_runtime, startup_bundle,
};
use super::stream::evict_streams_for_bundle;
use super::{
    NO_READY_SESSION_LEAD, PARTIAL_STARTUP_LEAD, RelayError, RelayResponse, StartupFailureRecord,
    fold_startup_failures,
};

/// Debounce window for coalescing rapid filesystem events. Long enough to ride
/// over an editor's write-temp-then-rename save sequence (so a single logical
/// edit reconciles once), short enough to feel responsive interactively.
const BUNDLE_WATCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// Interval at which reconciliation runs even though no filesystem event has
/// arrived.
///
/// Filesystem notification is best-effort on every platform: a backend can drop
/// events under load, and a debouncer can cancel a pair of them before they are
/// ever delivered — a file created and later removed reads as a file that never
/// existed, so neither change is reported. Because reconciliation re-scans every
/// layer from disk and diffs by fingerprint, an event is only ever a trigger to
/// look; nothing about the outcome depends on which event arrived. A periodic
/// sweep therefore turns a dropped trigger into a bounded delay rather than a
/// configuration change the relay never notices.
///
/// Short enough that a missed change resolves well inside an operator's patience,
/// long enough that the sweep's own cost — enumerating the layers and hashing
/// each definition — stays negligible against an idle relay.
const BUNDLE_WATCH_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

type BundleDebouncer = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Live bundle file watcher. Owns the debouncer (whose drop stops watching and
/// closes the event channel) and the consumer thread that runs reconciliation.
/// Dropping the watcher stops watching and joins the consumer thread.
pub struct BundleWatcher {
    // Dropped before the consumer is joined (see `Drop`): dropping the debouncer
    // closes the event channel so the consumer's receive loop terminates.
    debouncer: Option<BundleDebouncer>,
    consumer: Option<thread::JoinHandle<()>>,
}

impl Drop for BundleWatcher {
    fn drop(&mut self) {
        // Drop the debouncer first: this stops the filesystem watch and closes
        // the event channel, which ends the consumer's `for result in rx` loop.
        self.debouncer.take();
        if let Some(consumer) = self.consumer.take() {
            let _ = consumer.join();
        }
    }
}

/// Spawns the bundle file watcher over the bundles configuration directory.
///
/// The returned [`BundleWatcher`] must be held for as long as watching should
/// continue; dropping it tears the watcher down cleanly. Reconciliation runs on
/// a dedicated thread (filesystem and tmux operations are blocking), never on
/// the async runtime.
///
/// # Errors
///
/// Returns `RuntimeError` when the filesystem watcher cannot be created, the
/// bundles directory cannot be watched, or the consumer thread cannot be
/// spawned. The relay host treats this as non-fatal and continues serving
/// without dynamic reconciliation.
pub fn spawn_bundle_watcher(
    configuration_roots: &ConfigurationRoots,
    state_root: impl AsRef<Path>,
    catalog: BundleCatalog,
    no_autostart: bool,
) -> Result<BundleWatcher, RuntimeError> {
    let configuration_roots = configuration_roots.clone();
    let state_root = state_root.as_ref().to_path_buf();

    let (sender, receiver) = mpsc::channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(BUNDLE_WATCH_DEBOUNCE, None, sender).map_err(|source| {
        RuntimeError::validation(
            "runtime_bundle_watch_unavailable",
            format!("failed to create bundle file watcher: {source}"),
        )
    })?;
    // One recursive watch per layer, rather than per bundles directory, so a
    // bundles directory created after startup is observed without re-arming
    // anything. Every layer is watched because a change in any of them can
    // alter the effective set. Reconciliation is driven by the effective union
    // and by content fingerprints, so an event from an unrelated file under a
    // layer costs one re-scan and changes nothing.
    for layer in configuration_roots.layers() {
        debouncer
            .watch(layer, RecursiveMode::Recursive)
            .map_err(|source| {
                RuntimeError::validation(
                    "runtime_bundle_watch_unavailable",
                    format!(
                        "failed to watch configuration layer {}: {source}",
                        layer.display()
                    ),
                )
            })?;
    }

    let watched_roots = configuration_roots.clone();

    // Seed reconciliation state from the bundles already loaded at startup so the
    // first real change is diffed against their current on-disk content.
    let mut state = ReconcileState {
        fingerprints: seed_fingerprints(&configuration_roots, &catalog),
        failed: HashMap::new(),
    };

    let consumer = thread::Builder::new()
        .name("agentmux-bundle-watcher".to_string())
        .spawn(move || {
            run_bundle_watch_loop(&receiver, BUNDLE_WATCH_SWEEP_INTERVAL, |wake| match wake {
                WatchWake::Reconcile => {
                    reconcile_bundles(
                        &configuration_roots,
                        &state_root,
                        &catalog,
                        &mut state,
                        no_autostart,
                    );
                }
                WatchWake::Failed(cause) => {
                    emit_inscription("relay.bundle.watch.error", &json!({ "cause": cause }));
                }
            });
        })
        .map_err(|source| RuntimeError::io("spawn bundle watcher thread", source))?;

    emit_inscription(
        "relay.bundle.watch.started",
        &json!({ "configuration_layers": watched_roots.layers() }),
    );

    Ok(BundleWatcher {
        debouncer: Some(debouncer),
        consumer: Some(consumer),
    })
}

/// Why the watch loop woke.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchWake {
    /// Something may have changed on disk: re-scan the layers and reconcile.
    ///
    /// Carries no detail about the originating event, because reconciliation
    /// derives everything it needs from the filesystem. A sweep and a debounced
    /// event are the same wake for that reason.
    Reconcile,
    /// The watcher backend reported an error, rendered for inscription.
    Failed(String),
}

/// Runs the watch loop until the debouncer's channel closes, waking the caller
/// on every debounced change and at least once per `sweep_interval`.
///
/// Separated from [`spawn_bundle_watcher`] so the wake policy — which batches are
/// inert, and that a silent channel still reconciles — is exercisable without a
/// filesystem, a relay, or a real debouncer.
#[doc(hidden)]
pub fn run_bundle_watch_loop(
    receiver: &mpsc::Receiver<DebounceEventResult>,
    sweep_interval: Duration,
    mut on_wake: impl FnMut(WatchWake),
) {
    let mut sweep_deadline = Instant::now() + sweep_interval;
    loop {
        // Saturating: a wake whose handler outran the interval leaves the
        // deadline in the past, which asks for the next reconcile immediately
        // rather than for a negative wait.
        let wait = sweep_deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(Ok(events)) => {
                // Access (open/close/read) events cannot change bundle content,
                // and the watcher generates them itself: seeding and every
                // reconcile read the watched `.toml` files, so acting on
                // access-only batches would re-trigger reconciliation from its
                // own reads in a perpetual ~debounce-interval rescan loop. The
                // sweep deadline is deliberately left alone here — an inert batch
                // is not a reconcile, and must not postpone one.
                if events
                    .iter()
                    .all(|event| matches!(event.kind, EventKind::Access(_)))
                {
                    continue;
                }
                on_wake(WatchWake::Reconcile);
                sweep_deadline = Instant::now() + sweep_interval;
            }
            // An error is reported but does not reconcile, so it leaves the
            // deadline alone: whatever the backend failed to tell us is exactly
            // what the next sweep is for.
            Ok(Err(errors)) => on_wake(WatchWake::Failed(format!("{errors:?}"))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                on_wake(WatchWake::Reconcile);
                sweep_deadline = Instant::now() + sweep_interval;
            }
            // The debouncer was dropped: watching has stopped for good.
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Reconciliation bookkeeping carried across debounced notifications. Content
/// fingerprints distinguish a genuine bundle-file modification from filesystem
/// noise that leaves the file unchanged; the failed set suppresses repeated load
/// attempts (and repeated failure inscriptions) for an unchanged broken file.
struct ReconcileState {
    /// Content fingerprint of each loaded bundle's `.toml`, keyed by bundle name.
    fingerprints: HashMap<String, [u8; 32]>,
    /// Fingerprint of the last failed load attempt, keyed by bundle name.
    failed: HashMap<String, [u8; 32]>,
}

/// Re-scans every bundle layer and reconciles the effective union against the
/// loaded set.
///
/// Reconciliation is driven by change in the **effective** set, never by the
/// physical file event. A file appearing in an earlier layer over a loaded
/// bundle keeps the identifier in the union while changing which file supplies
/// it, so it reloads rather than unloading; deleting that file reveals the copy
/// in a later layer and reloads again; and editing a file which an earlier layer
/// still shadows changes no effective content, so it is inert.
fn reconcile_bundles(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    catalog: &BundleCatalog,
    state: &mut ReconcileState,
    no_autostart: bool,
) {
    // Enumeration is ground truth for what exists on disk, and the unload pass
    // below reads everything absent from it as deleted. A layer that cannot be
    // enumerated therefore has to stop reconciliation outright: an empty or
    // short result would be indistinguishable from the operator having removed
    // those bundles, and the relay would tear down a running catalog over a
    // permission bit. Faulting the host instead would be the same outage by
    // another route, so this holds the last successful reconciliation and waits
    // — the layer becoming readable again is itself a filesystem event, which
    // brings the next pass.
    let on_disk: HashSet<String> = match effective_bundle_definitions(configuration_roots) {
        Ok(definitions) => definitions.into_keys().collect(),
        Err(error) => {
            emit_inscription(
                "relay.bundle.reconcile_suppressed_unreadable_layer",
                &json!({ "cause": error.to_string() }),
            );
            return;
        }
    };
    let loaded = catalog.loaded_bundle_names();

    // Disappeared: loaded bundles whose file is no longer on disk.
    for bundle_name in loaded.difference(&on_disk) {
        unload_bundle(catalog, bundle_name, state);
    }

    // New or modified bundles present on disk.
    for bundle_name in &on_disk {
        let fingerprint = match fingerprint_bundle_file(configuration_roots, bundle_name) {
            Ok(fingerprint) => fingerprint,
            // The file vanished between scan and read; a later event reconciles it.
            Err(_) => continue,
        };
        if loaded.contains(bundle_name) {
            if state.fingerprints.get(bundle_name) == Some(&fingerprint) {
                continue;
            }
            if catalog.is_held(bundle_name) {
                // The bundle is held — the operator took it down, or it does not
                // autostart and was never brought up. Either way a configuration
                // edit must not silently start it. Absorb the new content
                // fingerprint so the edit is not re-detected on the next pass, but
                // leave the runtime stopped until an explicit `up` sets the intent
                // back to `Run`.
                state
                    .fingerprints
                    .insert(bundle_name.to_string(), fingerprint);
                emit_inscription(
                    "relay.bundle.reload_suppressed_held",
                    &json!({ "bundle_name": bundle_name }),
                );
                continue;
            }
            reload_bundle(
                configuration_roots,
                state_root,
                catalog,
                bundle_name,
                fingerprint,
                state,
            );
        } else {
            if state.failed.get(bundle_name) == Some(&fingerprint) {
                continue;
            }
            load_new_bundle(
                configuration_roots,
                state_root,
                catalog,
                bundle_name,
                fingerprint,
                state,
                no_autostart,
            );
        }
    }
}

/// Loads a newly detected bundle file. A bundle that autostarts (and the relay
/// was not launched with `--no-autostart`) is started; one that does not is
/// registered as `Hold` — known to the relay but not brought up, mirroring the
/// boot-time process-only path — so a later edit does not silently start it. A
/// validation or startup failure is recorded and the bundle is left unloaded;
/// other bundles continue serving.
fn load_new_bundle(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    catalog: &BundleCatalog,
    bundle_name: &str,
    fingerprint: [u8; 32],
    state: &mut ReconcileState,
    no_autostart: bool,
) {
    let paths = match BundleRuntimePaths::resolve(state_root, bundle_name) {
        Ok(paths) => paths,
        Err(source) => {
            record_load_failure(
                bundle_name,
                &source.to_string(),
                None,
                None,
                state,
                fingerprint,
            );
            return;
        }
    };
    if let Err(source) = ensure_bundle_runtime_directory(&paths) {
        record_load_failure(
            bundle_name,
            &source.to_string(),
            None,
            None,
            state,
            fingerprint,
        );
        return;
    }
    // The same rule applied at boot: a per-bundle `autostart = false` or a
    // relay-wide `--no-autostart` holds the bundle. A held bundle is registered
    // (its members become offline registry shells) but not started.
    let configuration = match load_bundle_configuration(configuration_roots, bundle_name) {
        Ok(configuration) => configuration,
        Err(source) => {
            record_load_failure(
                bundle_name,
                &source.to_string(),
                None,
                None,
                state,
                fingerprint,
            );
            return;
        }
    };
    if no_autostart || !configuration.autostart {
        if let Err(error) = register_configured_bundle_principals(&configuration) {
            record_load_failure(
                bundle_name,
                &error.message,
                Some(&error.code),
                error.details.as_ref(),
                state,
                fingerprint,
            );
            return;
        }
        catalog.insert(paths, HostingIntent::Hold);
        state
            .fingerprints
            .insert(bundle_name.to_string(), fingerprint);
        state.failed.remove(bundle_name);
        emit_inscription(
            "relay.bundle.loaded_held",
            &json!({
                "bundle_name": bundle_name,
                "reason": if no_autostart {
                    "relay_no_autostart"
                } else {
                    "bundle_autostart_disabled"
                },
            }),
        );
        return;
    }
    match startup_bundle(configuration_roots, &paths) {
        Ok(report) if report.ready_session_count > 0 => {
            catalog.insert(paths, HostingIntent::Run);
            state
                .fingerprints
                .insert(bundle_name.to_string(), fingerprint);
            state.failed.remove(bundle_name);
            emit_inscription(
                "relay.bundle.loaded",
                &json!({
                    "bundle_name": bundle_name,
                    "ready_session_count": report.ready_session_count,
                    "failed_session_count": report.failed_startups.len(),
                    "failed_sessions": partial_startup_detail(&report.failed_startups),
                }),
            );
        }
        Ok(report) => {
            record_no_ready_session_failure(
                bundle_name,
                &report.failed_startups,
                state,
                fingerprint,
            );
        }
        Err(error) => {
            record_load_failure(
                bundle_name,
                &error.message,
                Some(&error.code),
                error.details.as_ref(),
                state,
                fingerprint,
            );
        }
    }
}

/// Reloads a modified bundle: evict every active session, tear the runtime down,
/// then bring it back up with the new configuration. If the new configuration is
/// invalid the bundle is left unloaded with a recorded failure.
fn reload_bundle(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    catalog: &BundleCatalog,
    bundle_name: &str,
    fingerprint: [u8; 32],
    state: &mut ReconcileState,
) {
    let evicted_session_count =
        evict_streams_for_bundle(bundle_name, &bundle_reloaded_response(bundle_name));
    let paths = match BundleRuntimePaths::resolve(state_root, bundle_name) {
        Ok(paths) => paths,
        Err(source) => {
            catalog.remove(bundle_name);
            state.fingerprints.remove(bundle_name);
            record_load_failure(
                bundle_name,
                &source.to_string(),
                None,
                None,
                state,
                fingerprint,
            );
            return;
        }
    };
    // Tear the existing runtime down before reloading. A teardown *error* should
    // not block the attempt — it reports what tmux could not be asked, which the
    // restart may well fix.
    let teardown =
        shutdown_bundle_runtime(bundle_name, &paths.runtime_directory, &paths.tmux_socket);

    // A worker still registered here is different in kind, and must block the
    // restart. The startup pass is idempotent for a registered worker: it would
    // find the survivor's key, skip the spawn, and report the member ready on the
    // previous generation — which is exactly the inheritance this change exists
    // to end, reappearing in the window where a drain outlives the bounded wait.
    // The spec's "disappear followed by a new file" has not happened if the
    // disappear has not, so this is not a reload and must not be reported as one.
    //
    // Left unloaded with a recorded failure, matching the invalid-configuration
    // arm below: new connections fail fast rather than being served by a runtime
    // that is half torn down. The recorded failure is keyed by the supplying
    // file's fingerprint, which cannot tell this transient block from a permanent
    // validation error, so the bundle stays unloaded until that file changes
    // rather than being retried once the drain finishes.
    if let Ok(report) = teardown.as_ref()
        && report.unstopped_worker_count > 0
    {
        catalog.remove(bundle_name);
        state.fingerprints.remove(bundle_name);
        record_load_failure(
            bundle_name,
            "bundle teardown did not complete before reload; delivery workers from \
             the previous definition are still draining",
            Some("runtime_startup_failed"),
            Some(&json!({
                "unstopped_worker_count": report.unstopped_worker_count,
                "signalled_worker_count": report.signalled_worker_count,
                "evicted_session_count": evicted_session_count,
            })),
            state,
            fingerprint,
        );
        return;
    }

    match startup_bundle(configuration_roots, &paths) {
        Ok(report) if report.ready_session_count > 0 => {
            // Reload is reached only for a bundle whose intent is `Run` (held
            // bundles are suppressed above), so the refreshed entry stays `Run`.
            catalog.insert(paths, HostingIntent::Run);
            state
                .fingerprints
                .insert(bundle_name.to_string(), fingerprint);
            state.failed.remove(bundle_name);
            emit_inscription(
                "relay.bundle.reloaded",
                &json!({
                    "bundle_name": bundle_name,
                    "evicted_session_count": evicted_session_count,
                    "ready_session_count": report.ready_session_count,
                    "failed_session_count": report.failed_startups.len(),
                    "failed_sessions": partial_startup_detail(&report.failed_startups),
                }),
            );
        }
        outcome => {
            // The teardown already happened; a failed reload leaves the bundle
            // unloaded so new connections fail fast with validation_unknown_bundle.
            catalog.remove(bundle_name);
            state.fingerprints.remove(bundle_name);
            match outcome {
                Err(error) => record_load_failure(
                    bundle_name,
                    &error.message,
                    Some(&error.code),
                    error.details.as_ref(),
                    state,
                    fingerprint,
                ),
                Ok(report) => record_no_ready_session_failure(
                    bundle_name,
                    &report.failed_startups,
                    state,
                    fingerprint,
                ),
            }
        }
    }
}

/// Unloads a bundle whose file disappeared: evict every active session, remove it
/// from the catalog so new connections fail fast, and tear the runtime down.
fn unload_bundle(catalog: &BundleCatalog, bundle_name: &str, state: &mut ReconcileState) {
    let removed = catalog.remove(bundle_name);
    let evicted_session_count =
        evict_streams_for_bundle(bundle_name, &bundle_unloaded_response(bundle_name));
    if let Some(paths) = removed {
        let _ = shutdown_bundle_runtime(bundle_name, &paths.runtime_directory, &paths.tmux_socket);
    }
    state.fingerprints.remove(bundle_name);
    state.failed.remove(bundle_name);
    emit_inscription(
        "relay.bundle.unloaded",
        &json!({
            "bundle_name": bundle_name,
            "evicted_session_count": evicted_session_count,
        }),
    );
}

/// Records a load/reload attempt that hit no fatal error yet left no session
/// ready. Folds the per-session startup failure reasons (the same
/// [`fold_startup_failures`] the relay-host autostart summary uses) into the
/// recorded reason and structured detail, so the reload/watch surface names why
/// each session failed rather than emitting the blanket "nothing became ready"
/// placeholder. With no recorded failures (for example a bundle that configures
/// no sessions) it keeps the plain placeholder.
/// Returns the structured per-session failure detail for a load/reload that left
/// something ready, or `None` when nothing failed.
///
/// A load that succeeds partially is still a success, so it is not recorded as a
/// failed load — but the sessions that did fail have to appear somewhere, or the
/// only trace of them is a `ready_session_count` lower than the operator expected.
fn partial_startup_detail(failed_startups: &[StartupFailureRecord]) -> Option<serde_json::Value> {
    fold_startup_failures(PARTIAL_STARTUP_LEAD, failed_startups).map(|folded| folded.details)
}

fn record_no_ready_session_failure(
    bundle_name: &str,
    failed_startups: &[StartupFailureRecord],
    state: &mut ReconcileState,
    fingerprint: [u8; 32],
) {
    match fold_startup_failures(NO_READY_SESSION_LEAD, failed_startups) {
        Some(folded) => record_load_failure(
            bundle_name,
            &folded.reason,
            Some("runtime_startup_failed"),
            Some(&folded.details),
            state,
            fingerprint,
        ),
        None => record_load_failure(
            bundle_name,
            "no configured session reached ready state",
            Some("runtime_startup_failed"),
            None,
            state,
            fingerprint,
        ),
    }
}

fn record_load_failure(
    bundle_name: &str,
    reason: &str,
    code: Option<&str>,
    details: Option<&serde_json::Value>,
    state: &mut ReconcileState,
    fingerprint: [u8; 32],
) {
    state.failed.insert(bundle_name.to_string(), fingerprint);
    emit_inscription(
        "relay.bundle.load_failed",
        &json!({
            "bundle_name": bundle_name,
            "reason": reason,
            "code": code,
            "details": details,
        }),
    );
}

fn bundle_unloaded_response(bundle_name: &str) -> RelayResponse {
    RelayResponse::Error {
        error: RelayError {
            code: "runtime_bundle_unloaded".to_string(),
            message: "bundle configuration file was removed; the relay unloaded the bundle"
                .to_string(),
            details: Some(json!({ "bundle_name": bundle_name })),
        },
    }
}

fn bundle_reloaded_response(bundle_name: &str) -> RelayResponse {
    RelayResponse::Error {
        error: RelayError {
            code: "runtime_bundle_reloaded".to_string(),
            message: "bundle configuration file changed; the relay reloaded the bundle".to_string(),
            details: Some(json!({ "bundle_name": bundle_name })),
        },
    }
}

/// Seeds content fingerprints for the bundles already loaded at startup.
fn seed_fingerprints(
    configuration_roots: &ConfigurationRoots,
    catalog: &BundleCatalog,
) -> HashMap<String, [u8; 32]> {
    catalog
        .loaded_bundle_names()
        .into_iter()
        .filter_map(|bundle_name| {
            fingerprint_bundle_file(configuration_roots, &bundle_name)
                .ok()
                .map(|fingerprint| (bundle_name, fingerprint))
        })
        .collect()
}

/// Computes the SHA-256 fingerprint of a bundle definition: the resolved path
/// which supplies it, then its content.
///
/// Content alone cannot distinguish a definition appearing in an earlier layer
/// over a byte-identical one — or being deleted to reveal it — from no change at
/// all. Both change which file the relay tracks for that identifier, so both
/// must reload.
fn fingerprint_bundle_file(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> io::Result<[u8; 32]> {
    // An unreadable layer arrives here as an ordinary error, which the caller
    // already treats as "leave this bundle as it is and reconcile on a later
    // event" — the same retention the enumeration fault gets, reached by the
    // path that was already there.
    let path = bundle_configuration_path(configuration_roots, bundle_name)
        .map_err(|source| io::Error::other(source.to_string()))?;
    let bytes = std::fs::read(&path)?;
    let mut hasher = Sha256::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    // Separator: without it a path/content split could shift without altering
    // the concatenation.
    hasher.update([0u8]);
    hasher.update(&bytes);
    Ok(hasher.finalize().into())
}
