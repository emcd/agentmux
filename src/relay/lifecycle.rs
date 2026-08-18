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
    BundleStartupReport, ReconciliationReport, RelayError, ShutdownReport, StartupFailureRecord,
    map_config, map_tui_config, relay_error,
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
    let bundle = load_hosted_bundle(configuration_roots, paths)?;
    let authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    reconcile_loaded_bundle(&bundle, paths, choices_pending_max(&authorization))
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

/// Stamps `bundle` for the relay hosting `paths`, overwriting
/// `AGENTMUX_STATE_DIRECTORY` in every member with that relay's state root.
///
/// The single definition of that overwrite, applied wherever a bundle is
/// prepared on a path that can end in a spawn. There are three such paths and
/// they must agree: first startup, `up`/reconcile, and the lazy Pty spawn a
/// delivery triggers, which loads its own copy of the bundle at delivery time. A
/// member spawned by one and a member spawned by another must be pointed at the
/// same relay, or which path an operator happened to take decides whether the
/// child can find it.
///
/// Takes the bundle by value and requires the hosting relay's
/// [`BundleRuntimePaths`] rather than a bare state root: a caller holding an
/// unstamped configuration cannot reach a spawn without moving it through here,
/// and cannot call this without having established which relay is hosting.
/// Idempotent, so a bundle that arrives already stamped is unharmed.
pub(super) fn stamp_hosted_bundle(
    mut bundle: BundleConfiguration,
    paths: &BundleRuntimePaths,
) -> BundleConfiguration {
    for member in &mut bundle.members {
        inject_spawn_state_directory(&mut member.environment, paths.state_root.as_path());
    }
    bundle
}

/// Loads the bundle named by `paths`, stamped for the relay hosting it.
///
/// Every relay path that loads a bundle it may spawn from loads through here.
/// The signature is what carries the requirement: the authoritative paths are
/// not optional and there is no name-only form, so a new spawn path cannot be
/// written that silently keeps a member's own state-root declaration.
///
/// # Errors
///
/// Returns the structured configuration errors bundle loading raises.
pub(super) fn load_hosted_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<BundleConfiguration, RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, paths.bundle_name.as_str())
        .map_err(map_config)?;
    Ok(stamp_hosted_bundle(bundle, paths))
}

/// Returns `bundle`'s members prepared for spawning by the relay owning
/// `paths`.
fn members_for_spawn(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
) -> Vec<BundleMember> {
    stamp_hosted_bundle(bundle.clone(), paths).members
}

pub(super) fn startup_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<BundleStartupReport, RelayError> {
    let bundle = load_hosted_bundle(configuration_roots, paths)?;
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

/// Reconciles `bundle`'s configured sessions against tmux state and reports what
/// changed alongside what is ready afterward.
///
/// A configured session that fails to be created does not abort the pass: a
/// bundle is a set of sessions, and one failing is not a reason to withhold the
/// rest — the same tolerance [`startup_loaded_bundle`] already applies. Failures
/// that are not attributable to one session (a tmux state query, a prune, the
/// principal registration) still fail the whole operation, because a pass that
/// cannot say what it did or did not do has nothing to report.
pub(super) fn reconcile_loaded_bundle(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
    choices_pending_max: usize,
) -> Result<ReconciliationReport, RelayError> {
    let tmux_socket = paths.tmux_socket.as_path();
    let runtime_directory = paths.runtime_directory.as_path();
    // Refresh the static registry shells for every configured member so the
    // reconcile/`up` path keeps the unified registry complete (offline members
    // included), independent of transport readiness.
    register_configured_bundle_principals(bundle)?;
    let mut members = members_for_spawn(bundle, paths);
    members.sort_by(|left, right| left.id.cmp(&right.id));
    let configured_sessions = members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();
    let missing = members
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
        match create_member_with_retry(tmux_socket, &bootstrap_member) {
            Ok(()) => {
                report.bootstrap_session = Some(bootstrap_member.id.clone());
                report.created_sessions.push(bootstrap_member.id.clone());
            }
            Err(error) => report
                .failed_startups
                .push(creation_failure_record(&bootstrap_member, &error)),
        }
    }

    let remaining = missing.into_iter().skip(1).collect::<Vec<_>>();
    if !remaining.is_empty() {
        let mut handles = Vec::with_capacity(remaining.len());
        for member in remaining {
            let tmux_socket = tmux_socket.to_path_buf();
            handles.push(thread::spawn(move || {
                match create_member_with_retry(&tmux_socket, &member) {
                    Ok(()) => Ok(member.id.clone()),
                    // Boxed so the failure path does not widen every worker's
                    // result to the size of a whole failure record.
                    Err(error) => Err(Box::new(creation_failure_record(&member, &error))),
                }
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(Ok(created_session)) => report.created_sessions.push(created_session),
                Ok(Err(failure)) => report.failed_startups.push(*failure),
                // A panicked worker leaves the pass unable to say whether that
                // member was created, so it stays a whole-operation failure
                // rather than being folded in as a per-session one.
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

    // Every configured member is brought up here, not only the ones the tmux
    // phase above created: a member that was already running may not be ready, a
    // member this pass created may not have become ready, and a member with no
    // tmux session to create at all — an ACP target — has not been started yet.
    // Counting creations instead would call a bundle healthy on the strength of
    // `new-session` returning, and would report a target `up` never tried to
    // start as one that failed.
    let pending = members
        .iter()
        .filter(|member| {
            !report
                .failed_startups
                .iter()
                .any(|failure| failure.session_id == member.id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let (ready_session_count, mut failed_startups) = startup_members(
        bundle.bundle_name.as_str(),
        runtime_directory,
        tmux_socket,
        &pending,
        choices_pending_max,
    )?;
    report.ready_session_count = ready_session_count;
    report.failed_startups.append(&mut failed_startups);

    report.failed_startups = crate::relay::persist_startup_failures(
        bundle.bundle_name.as_str(),
        runtime_directory,
        &report.failed_startups,
    )
    .map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to persist startup failure history during reconciliation",
            Some(json!({ "bundle_name": bundle.bundle_name, "cause": cause })),
        )
    })?;

    let _ = cleanup_tmux_server_when_unowned(tmux_socket)?;
    Ok(report)
}

/// Records a member the reconcile pass could not create.
fn creation_failure_record(
    member: &BundleMember,
    error: &TmuxLifecycleError,
) -> StartupFailureRecord {
    StartupFailureRecord {
        session_id: member.id.clone(),
        transport: member.target.session_type().into(),
        code: "runtime_startup_failed".to_string(),
        reason: "failed to create tmux session during reconciliation".to_string(),
        timestamp: startup_timestamp(),
        sequence: 0,
        details: Some(json!({
            "session_id": member.id,
            "cause": error.message,
            "error_code": error.code,
        })),
    }
}

/// Records a per-member bring-up failure.
fn member_failure_record(
    member: &BundleMember,
    (code, reason, details): (String, String, Option<serde_json::Value>),
) -> StartupFailureRecord {
    StartupFailureRecord {
        session_id: member.id.clone(),
        transport: member.target.session_type().into(),
        code,
        reason,
        timestamp: startup_timestamp(),
        sequence: 0,
        details,
    }
}

/// Brings one configured member to ready state, or reports why it could not.
///
/// The single per-member bring-up step, shared by the startup pass and the
/// reconcile/`up` pass. The two have to agree about what starting a member
/// *means*, not merely about how to observe one: a transport the startup path
/// initializes but the reconcile path only inspects would have `up` report the
/// member failed for the sole reason that `up` never tried to start it.
///
/// Idempotent for every transport — a tmux member whose session exists is only
/// checked for readiness, and an ACP member whose worker is already registered is
/// left alone — so a repeated pass over a healthy bundle changes nothing.
fn startup_member(
    bundle_name: &str,
    runtime_directory: &Path,
    tmux_socket: &Path,
    member: &BundleMember,
    choices_pending_max: usize,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    match &member.target {
        TargetConfiguration::Tmux(_) => startup_tmux_member(tmux_socket, member),
        TargetConfiguration::Acp(_) => initialize_acp_target_for_startup(
            bundle_name,
            runtime_directory,
            member,
            choices_pending_max,
        ),
        // Pty startup is recorded as ready when the per-coder config
        // resolves cleanly. The bootstrap path constructs the Pty
        // transport lazily when the dispatcher first references it;
        // for the v1 startup-count accounting we record Pty members
        // as ready alongside Tmux / ACP. The full Pty bootstrap
        // path lands alongside the bootstrap-side refactor
        // (referenced from the add-pty-transport OpenSpec §8
        // worker readiness; not implemented in this commit).
        TargetConfiguration::Pty(_) => Ok(()),
        // `ui`/`pubsub` members have no implemented startup path; record a
        // structured startup failure and exclude them from active routing.
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => Err((
            "runtime_session_type_not_implemented".to_string(),
            "session type delivery is not yet implemented".to_string(),
            None,
        )),
    }
}

/// Brings up each member of `members`, returning how many reached ready state and
/// a record for each that did not.
fn startup_members(
    bundle_name: &str,
    runtime_directory: &Path,
    tmux_socket: &Path,
    members: &[BundleMember],
    choices_pending_max: usize,
) -> Result<(usize, Vec<StartupFailureRecord>), RelayError> {
    let mut ready_session_count = 0usize;
    let mut failed_startups = Vec::<StartupFailureRecord>::new();
    for member in members {
        match startup_member(
            bundle_name,
            runtime_directory,
            tmux_socket,
            member,
            choices_pending_max,
        ) {
            Ok(()) => {
                clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                ready_session_count += 1;
            }
            Err(error) => failed_startups.push(member_failure_record(member, error)),
        }
    }
    Ok((ready_session_count, failed_startups))
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

    let mut members = members_for_spawn(bundle, paths);
    members.sort_by(|left, right| left.id.cmp(&right.id));

    let (ready_session_count, failed_startups) = startup_members(
        bundle.bundle_name.as_str(),
        runtime_directory,
        tmux_socket,
        &members,
        choices_pending_max,
    )?;

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
