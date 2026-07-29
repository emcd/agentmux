use std::{collections::HashSet, path::Path, thread};

use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::configuration::{
    BundleConfiguration, BundleMember, ConfigurationRoots, TargetConfiguration,
    inject_spawn_state_directory, load_bundle_configuration, load_tui_configuration,
};
use crate::runtime::paths::BundleRuntimePaths;

use super::identity::canonical_session_id;
use super::startup_state::note_session_served_successfully;
use super::stream::register_configured_session;
use super::{
    BundleStartupReport, ListedSessionTransport, ReconciliationReport, RelayError, ShutdownReport,
    StartupFailureRecord, map_config, map_tui_config, relay_error,
};
use crate::relay::authorization::{choices_pending_max, load_authorization_context};
use crate::relay::delivery::initialize_acp_target_for_startup;
use crate::tmux::lifecycle::{
    TmuxLifecycleError, cleanup_tmux_server_when_unowned, create_member_with_retry,
    list_owned_sessions, prune_owned_session, session_exists, startup_tmux_member,
};

impl From<TmuxLifecycleError> for RelayError {
    fn from(error: TmuxLifecycleError) -> Self {
        relay_error(error.code.as_str(), error.message.as_str(), error.details)
    }
}

/// Reconciles configured bundle sessions against tmux state.
///
/// # Errors
///
/// Returns structured validation/configuration errors when bundle loading
/// fails, and internal failures when tmux session operations fail.
pub(super) fn reconcile_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<ReconciliationReport, RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, paths.bundle_name.as_str())
        .map_err(map_config)?;
    let _authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    reconcile_loaded_bundle(&bundle, paths)
}

/// Validates a bundle's configuration exactly as startup does — bundle and
/// coders schema plus authorization-policy resolution (`policies.toml`,
/// `relay.toml`, and `users.toml` policy mappings) — without touching tmux or
/// relay runtime state. Backs the `agentmux check configuration` pre-flight
/// command: it shares the `load_bundle_configuration` + `load_authorization_context`
/// path so a pre-flight catches exactly what a live startup would reject.
///
/// # Errors
///
/// Returns the same structured validation/configuration errors as startup when
/// any artifact fails to parse or resolve.
pub(super) fn preflight_bundle_configuration(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> Result<(), RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, bundle_name).map_err(map_config)?;
    let _authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    Ok(())
}

/// Prunes managed sessions and reaps tmux server when safe during shutdown.
///
/// # Errors
///
/// Returns internal failures when tmux session operations fail.
pub(super) fn shutdown_bundle_runtime(tmux_socket: &Path) -> Result<ShutdownReport, RelayError> {
    let mut report = ShutdownReport::default();
    let mut owned_sessions = list_owned_sessions(tmux_socket)?;
    owned_sessions.sort();
    for session_name in owned_sessions {
        prune_owned_session(tmux_socket, &session_name)?;
        report.pruned_sessions.push(session_name);
    }
    report.killed_tmux_server = cleanup_tmux_server_when_unowned(tmux_socket)?;
    Ok(report)
}

/// Returns `bundle`'s members prepared for spawning by the relay owning
/// `paths`.
///
/// The relay's state root is injected authoritatively into each member's
/// environment. Both bring-up paths — first startup and `up`/reconcile — run
/// through here, because a member created by one and a member created by the
/// other must be pointed at the same relay; injecting on only one of them would
/// leave whichever path an operator happened to take deciding whether the child
/// could find its relay.
fn members_for_spawn(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
) -> Vec<BundleMember> {
    let mut members = bundle.members.clone();
    for member in &mut members {
        inject_spawn_state_directory(&mut member.environment, paths.state_root.as_path());
    }
    members
}

pub(super) fn startup_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<BundleStartupReport, RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, paths.bundle_name.as_str())
        .map_err(map_config)?;
    let authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    startup_loaded_bundle(&bundle, paths, choices_pending_max(&authorization))
}

/// Registers the relay-wide principals declared in `users.toml` as static
/// (offline) unified-registry entries at relay startup.
///
/// A declared relay-wide principal (e.g. an operator `@GLOBAL` UI session) is a
/// known target whether or not it is connected: offline is a state, not absence.
/// Registering it here lets look/raww resolve its capability from the registry —
/// a declared-but-disconnected principal sorts as `validation_unsupported_operation`
/// (capability gate) rather than `validation_unknown_target` — and a Hello later
/// flips the same entry online. Absent `users.toml` is not an error.
pub(super) fn register_configured_relay_wide_principals(
    configuration_roots: &ConfigurationRoots,
) -> Result<(), RelayError> {
    let Some(users) = load_tui_configuration(configuration_roots).map_err(map_tui_config)? else {
        return Ok(());
    };
    for session in &users.sessions {
        register_configured_session(session.id.as_str(), session.session_type).map_err(
            |error| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to register configured relay-wide principal in the unified registry",
                    Some(json!({ "session_id": session.id, "cause": error.to_string() })),
                )
            },
        )?;
    }
    Ok(())
}

/// Registers every configured member of `bundle` as a static (routing/capability)
/// unified-registry entry, without starting any transport.
///
/// The registry must hold every known principal — offline is a state, not absence
/// — so look/raww/list resolve a declared-but-not-yet-ready member from its entry
/// rather than treating it as an unknown target. Re-registration is idempotent: it
/// refreshes the static shell and preserves any stream state already attached by a
/// live connection.
pub(super) fn register_configured_bundle_principals(
    bundle: &BundleConfiguration,
) -> Result<(), RelayError> {
    for member in &bundle.members {
        register_configured_session(
            canonical_session_id(member.id.as_str(), bundle.bundle_name.as_str()).as_str(),
            member.target.session_type(),
        )
        .map_err(|error| {
            relay_error(
                "internal_unexpected_failure",
                "failed to register configured session in the unified registry",
                Some(json!({ "session_id": member.id, "cause": error.to_string() })),
            )
        })?;
    }
    Ok(())
}

/// Loads `bundle_name` and registers its configured members as static registry
/// shells without starting transports.
///
/// Used by the process-only / `--no-autostart` host path, where no `startup` or
/// `reconcile` runs: the registry still holds every configured principal before
/// the relay begins serving, so a declared-but-offline member is a known target
/// rather than an unknown one.
pub(super) fn register_configured_bundle(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> Result<(), RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, bundle_name).map_err(map_config)?;
    register_configured_bundle_principals(&bundle)
}

pub(super) fn reconcile_loaded_bundle(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
) -> Result<ReconciliationReport, RelayError> {
    let tmux_socket = paths.tmux_socket.as_path();
    // Refresh the static registry shells for every configured member so the
    // reconcile/`up` path keeps the unified registry complete (offline members
    // included), independent of transport readiness.
    register_configured_bundle_principals(bundle)?;
    let members = members_for_spawn(bundle, paths);
    let configured_sessions = members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();
    let mut missing = members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .filter_map(|member| match session_exists(tmux_socket, &member.id) {
            Ok(true) => None,
            Ok(false) => Some(Ok(member.clone())),
            Err(reason) => Some(Err(relay_error(
                "internal_unexpected_failure",
                "failed to query tmux session state during reconciliation",
                Some(json!({"session_name": member.id, "cause": reason})),
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    missing.sort_by(|left, right| left.id.cmp(&right.id));

    let mut report = ReconciliationReport::default();

    let mut stale_owned = list_owned_sessions(tmux_socket)?
        .into_iter()
        .filter(|session_name| !configured_sessions.contains(session_name))
        .collect::<Vec<_>>();
    stale_owned.sort();
    for session_name in stale_owned {
        prune_owned_session(tmux_socket, &session_name)?;
        report.pruned_sessions.push(session_name);
    }

    if let Some(bootstrap_member) = missing.first().cloned() {
        create_member_with_retry(tmux_socket, &bootstrap_member)?;
        report.bootstrap_session = Some(bootstrap_member.id.clone());
        report.created_sessions.push(bootstrap_member.id.clone());
    }

    let remaining = missing.into_iter().skip(1).collect::<Vec<_>>();
    if !remaining.is_empty() {
        let mut handles = Vec::with_capacity(remaining.len());
        for member in remaining {
            let tmux_socket = tmux_socket.to_path_buf();
            handles.push(thread::spawn(move || {
                create_member_with_retry(&tmux_socket, &member).map(|_| member.id.clone())
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(Ok(created_session)) => report.created_sessions.push(created_session),
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => {
                    return Err(relay_error(
                        "internal_unexpected_failure",
                        "reconciliation worker thread panicked",
                        None,
                    ));
                }
            }
        }
    }

    let _ = cleanup_tmux_server_when_unowned(tmux_socket)?;
    Ok(report)
}

fn startup_loaded_bundle(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
    choices_pending_max: usize,
) -> Result<BundleStartupReport, RelayError> {
    let runtime_directory = paths.runtime_directory.as_path();
    let tmux_socket = paths.tmux_socket.as_path();
    let configured_tmux_sessions = bundle
        .members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();

    let mut stale_owned = list_owned_sessions(tmux_socket)?
        .into_iter()
        .filter(|session_name| !configured_tmux_sessions.contains(session_name))
        .collect::<Vec<_>>();
    stale_owned.sort();
    for session_name in stale_owned {
        prune_owned_session(tmux_socket, &session_name)?;
    }

    // Register a static unified-registry entry for every configured member
    // before attempting startup, so look/raww resolve the target's capabilities
    // from the registry and an offline-but-declared principal is a known target
    // (not an unknown one). The entries persist independently of transport
    // readiness; a Hello later flips a member online.
    register_configured_bundle_principals(bundle)?;

    let mut ready_session_count = 0usize;
    let mut failed_startups = Vec::<StartupFailureRecord>::new();
    let mut members = members_for_spawn(bundle, paths);
    members.sort_by(|left, right| left.id.cmp(&right.id));

    for member in members {
        match &member.target {
            TargetConfiguration::Tmux(_) => match startup_tmux_member(tmux_socket, &member) {
                Ok(()) => {
                    clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                    ready_session_count += 1;
                }
                Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                    session_id: member.id.clone(),
                    transport: ListedSessionTransport::Tmux,
                    code,
                    reason,
                    timestamp: startup_timestamp(),
                    sequence: 0,
                    details,
                }),
            },
            TargetConfiguration::Acp(_) => {
                match initialize_acp_target_for_startup(
                    bundle.bundle_name.as_str(),
                    runtime_directory,
                    &member,
                    choices_pending_max,
                ) {
                    Ok(()) => {
                        clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                        ready_session_count += 1;
                    }
                    Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                        session_id: member.id.clone(),
                        transport: ListedSessionTransport::Acp,
                        code,
                        reason,
                        timestamp: startup_timestamp(),
                        sequence: 0,
                        details,
                    }),
                }
            }
            // Pty startup is recorded as ready when the per-coder config
            // resolves cleanly. The bootstrap path constructs the Pty
            // transport lazily when the dispatcher first references it;
            // for the v1 startup-count accounting we record Pty members
            // as ready alongside Tmux / ACP. The full Pty bootstrap
            // path lands alongside the bootstrap-side refactor
            // (referenced from the add-pty-transport OpenSpec §8
            // worker readiness; not implemented in this commit).
            TargetConfiguration::Pty(_) => {
                clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                ready_session_count += 1;
            }
            // `ui`/`pubsub` members have no implemented startup path; record a
            // structured startup failure and exclude them from active routing.
            TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
                failed_startups.push(StartupFailureRecord {
                    session_id: member.id.clone(),
                    transport: member.target.session_type().into(),
                    code: "runtime_session_type_not_implemented".to_string(),
                    reason: "session type delivery is not yet implemented".to_string(),
                    timestamp: startup_timestamp(),
                    sequence: 0,
                    details: None,
                });
            }
        }
    }

    Ok(BundleStartupReport {
        ready_session_count,
        failed_startups,
    })
}

fn clear_session_startup_failures(
    runtime_directory: &Path,
    session_id: &str,
) -> Result<(), RelayError> {
    note_session_served_successfully(runtime_directory, session_id).map_err(|reason| {
        relay_error(
            "internal_unexpected_failure",
            "failed to clear startup failure history after successful session startup",
            Some(json!({"session_id": session_id, "cause": reason})),
        )
    })
}

fn startup_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
