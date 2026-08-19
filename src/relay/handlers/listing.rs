use std::path::Path;

use serde_json::json;

use crate::{
    configuration::{BundleConfiguration, TargetConfiguration},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
};

use super::super::authorization::{AuthorizationContext, RouteAuthorization, choices_pending_max};
use super::super::delivery::acp_session_is_ready;
use super::super::lifecycle::{reconcile_loaded_bundle, shutdown_bundle_runtime};
use super::super::routing::{
    Addressing, Capability, OperationProfile, requester_home_namespace, resolve_list_route,
};
use super::super::{
    BundleCatalog, BundleTransitionEntry, HostingIntent, ListedBundle, ListedBundleStartupHealth,
    ListedBundleState, ListedSession, NO_READY_SESSION_LEAD, PARTIAL_STARTUP_LEAD, RelayError,
    RelayResponse, SCHEMA_VERSION, StartupFailureRecord, canonical_session_id,
    fold_startup_failures, load_startup_failures, relay_error,
};
use super::routed::run_target_operation;
use crate::tmux::pane::resolve_active_pane_target;

pub(super) fn handle_bundle_up(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    // Bringing the bundle up sets its hosting intent to `Run` so the watcher
    // resumes reloading (and starting) it on configuration edits. Set
    // unconditionally (even on an already-hosted no-op, and even for a bundle
    // whose `autostart = false` seeded it `Hold`) because the operator's request
    // to host it is the authoritative signal, regardless of current runtime state.
    catalog.set_intent(bundle.bundle_name.as_str(), HostingIntent::Run);
    // From the catalog rather than derived from `runtime_directory`: the entry
    // carries the state root the hosting relay actually resolved, and that is
    // the root every member it spawns has to be pointed at.
    let paths = catalog.lookup(bundle.bundle_name.as_str()).ok_or_else(|| {
        relay_error(
            "internal_unexpected_failure",
            "bundle is not present in the runtime catalog",
            Some(json!({ "bundle_name": bundle.bundle_name })),
        )
    })?;
    // The reconcile pass starts every configured member, ACP targets included,
    // so it needs the same pending-choice bound the startup path passes.
    let report = reconcile_loaded_bundle(bundle, &paths, choices_pending_max(authorization))?;
    let changed = report.bootstrap_session.is_some()
        || !report.created_sessions.is_empty()
        || !report.pruned_sessions.is_empty();
    // The outcome comes from readiness, not from what the pass created: a bundle
    // whose sessions were all already running and ready changed nothing and is
    // still up, while a session that was created but never became ready is a
    // failure. `changed` continues to carry creation and pruning alone, so the
    // idempotent `skipped` result keeps its existing meaning.
    let bundle_result = if report.failed_startups.is_empty() {
        if changed {
            BundleTransitionEntry {
                bundle_name: bundle.bundle_name.clone(),
                outcome: "hosted".to_string(),
                reason_code: None,
                reason: None,
                details: None,
            }
        } else {
            BundleTransitionEntry {
                bundle_name: bundle.bundle_name.clone(),
                outcome: "skipped".to_string(),
                reason_code: Some("already_hosted".to_string()),
                reason: Some("bundle runtime is already hosted".to_string()),
                details: None,
            }
        }
    } else {
        partially_started_entry(
            bundle.bundle_name.as_str(),
            report.ready_session_count,
            &report.failed_startups,
        )
    };
    let degraded = bundle_result.outcome == "degraded";
    let failed = bundle_result.outcome == "failed";
    let hosted = bundle_result.outcome == "hosted";
    Ok(RelayResponse::BundleTransition {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "up".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(hosted),
        degraded_bundle_count: usize::from(degraded),
        skipped_bundle_count: usize::from(!hosted && !degraded && !failed),
        failed_bundle_count: usize::from(failed),
        changed_any: hosted || degraded,
    })
}

/// Builds the transition entry for a bundle whose bring-up recorded per-session
/// failures: `degraded` when something is ready, `failed` when nothing is.
///
/// `degraded` is the spelling `bundle-lifecycle`'s startup-health model already
/// uses for "at least one session ready and at least one startup attempt failed",
/// and it is used here under that same predicate — `ready_session_count` comes
/// from the shared readiness evaluation, not from what the pass created.
fn partially_started_entry(
    bundle_name: &str,
    ready_session_count: usize,
    failed_startups: &[StartupFailureRecord],
) -> BundleTransitionEntry {
    let degraded = ready_session_count > 0;
    let lead = if degraded {
        PARTIAL_STARTUP_LEAD
    } else {
        NO_READY_SESSION_LEAD
    };
    let (reason, details) = match fold_startup_failures(lead, failed_startups) {
        Some(folded) => (folded.reason, Some(folded.details)),
        None => (lead.to_string(), None),
    };
    BundleTransitionEntry {
        bundle_name: bundle_name.to_string(),
        outcome: if degraded { "degraded" } else { "failed" }.to_string(),
        reason_code: Some("runtime_startup_failed".to_string()),
        reason: Some(reason),
        details,
    }
}

pub(super) fn handle_bundle_down(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
    catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    // Set the hosting intent to `Hold` before tearing the runtime down so a
    // subsequent configuration edit does not let the watcher silently restart
    // the bundle. Set unconditionally (even on an already-unhosted no-op) because
    // the operator's request to keep it down is the authoritative signal.
    catalog.set_intent(bundle.bundle_name.as_str(), HostingIntent::Hold);
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    let report = shutdown_bundle_runtime(
        bundle.bundle_name.as_str(),
        runtime_directory,
        tmux_socket.as_path(),
    )?;
    // Derived from every transport the teardown ended, not from tmux alone. A
    // bundle whose members are all ACP prunes no tmux session and reaps no
    // server, and reporting that as `already_unhosted` told the operator nothing
    // had been running while its agents were still alive.
    //
    // Read from what the teardown *asked* rather than from what it watched leave.
    // A worker whose drain outlives the bounded wait was still stopped, and a
    // bundle whose workers are draining is not one that was already unhosted —
    // deriving this from completions would reinstate the same misreport in the
    // narrower window where the fence times out.
    let changed = !report.pruned_sessions.is_empty()
        || report.killed_tmux_server
        || report.signalled_worker_count > 0;
    let bundle_result = if changed {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "unhosted".to_string(),
            reason_code: None,
            reason: None,
            details: None,
        }
    } else {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "skipped".to_string(),
            reason_code: Some("already_unhosted".to_string()),
            reason: Some("bundle runtime is already unhosted".to_string()),
            details: None,
        }
    };
    Ok(RelayResponse::BundleTransition {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "down".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(changed),
        // `down` has no per-session failure concept: a session either got pruned
        // or was already gone.
        degraded_bundle_count: 0,
        skipped_bundle_count: usize::from(!changed),
        failed_bundle_count: 0,
        changed_any: changed,
    })
}

/// Lists a bundle's sessions through the uniform routing/authorization spine.
///
/// `dispatch_bundle` carries the authorization context in which the requester's
/// `list` control resolves (its home bundle for a session; for a relay-wide
/// principal, the enumerated bundle, whose context replicates the TUI-config
/// controls). The requester's *home namespace* — used for the cross-namespace
/// tier — is derived from its principal id, so a relay-wide principal's home is
/// `GLOBAL`, not whichever bundle it is listing. `enumerate_bundle` is the bundle
/// whose sessions are listed: the same bundle for a same-namespace list, or a
/// peer bundle for a cross-namespace list. A cross-namespace enumeration requires
/// the requester's `list` scope to reach the `all` tier; a same-namespace
/// enumeration needs only `home`.
pub(in crate::relay) fn handle_list_routed(
    dispatch_bundle: &BundleConfiguration,
    dispatch_authorization: &AuthorizationContext,
    enumerate_bundle: &BundleConfiguration,
    enumerate_runtime_directory: &Path,
    requester_session: Option<String>,
) -> Result<RelayResponse, RelayError> {
    let tmux_socket = tmux_socket_path_for_runtime_directory(enumerate_runtime_directory);
    let requester_session = requester_session.ok_or_else(|| {
        relay_error(
            "validation_unknown_sender",
            "requester_session is required for list authorization",
            None,
        )
    })?;
    let sender = super::sender::resolve_sender_identity(
        dispatch_bundle,
        dispatch_authorization,
        requester_session.as_str(),
        "requester_session",
    )?;
    // List names no per-session target, so the spine's `prepare` stage is empty;
    // existence of the enumerate bundle is guaranteed by the connection layer. The
    // requester's `list` control is resolved in the dispatch bundle while the
    // route's home namespace drives the cross-namespace tier.
    run_target_operation(
        dispatch_bundle.bundle_name.as_str(),
        RouteAuthorization::Policy(dispatch_authorization),
        OperationProfile {
            capability: Capability::List,
            addressing: Addressing::BundleEnumerate,
        },
        || {
            Ok(resolve_list_route(
                requester_home_namespace(
                    sender.session_id.as_str(),
                    dispatch_bundle.bundle_name.as_str(),
                ),
                sender.session_id.as_str(),
                enumerate_bundle.bundle_name.as_str(),
            ))
        },
        |_route| Ok(()),
        |_route, ()| {
            execute_list(
                enumerate_bundle,
                enumerate_runtime_directory,
                tmux_socket.as_path(),
                sender.session_id.as_str(),
            )
        },
    )
}

/// Enumerates a bundle's sessions and builds the list response. Runs as the
/// spine's `execute` stage, after authorization.
fn execute_list(
    enumerate_bundle: &BundleConfiguration,
    enumerate_runtime_directory: &Path,
    tmux_socket: &Path,
    requester_session_id: &str,
) -> Result<RelayResponse, RelayError> {
    let bundle = build_listed_bundle(enumerate_bundle, enumerate_runtime_directory, tmux_socket)?;
    emit_inscription(
        "relay.list.response",
        &json!({
            "namespace": bundle.id,
            "requester_session": requester_session_id,
            "hosted": bundle.hosted,
            "state": bundle.state,
            "startup_health": bundle.startup_health,
            "startup_failure_count": bundle.startup_failure_count,
            "principal_count": bundle.principals.len(),
        }),
    );
    Ok(RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle,
    })
}

/// Builds the canonical listed-bundle payload for `enumerate_bundle`, folding
/// per-session readiness and bundle startup state.
///
/// The single builder for a bundle's list projection: the routed `List` execute
/// stage wraps it into a `RelayResponse::List`, and cross-relay principal
/// discovery reuses it and then scope-filters the returned `principals` — so
/// discovery never re-derives readiness or state folding. The returned bundle
/// always carries `principals_partial: None`; a filtered caller sets the marker
/// itself after trimming principals.
pub(in crate::relay) fn build_listed_bundle(
    enumerate_bundle: &BundleConfiguration,
    enumerate_runtime_directory: &Path,
    tmux_socket: &Path,
) -> Result<ListedBundle, RelayError> {
    let sessions = enumerate_bundle
        .members
        .iter()
        .map(|member| ListedSession {
            id: canonical_session_id(member.id.as_str(), enumerate_bundle.bundle_name.as_str()),
            name: member.name.clone(),
            transport: member.target.session_type().into(),
            ready: session_ready_for_list(
                enumerate_bundle,
                enumerate_runtime_directory,
                tmux_socket,
                member,
            ),
        })
        .collect::<Vec<_>>();

    let recent_startup_failures =
        load_startup_failures(enumerate_runtime_directory).map_err(|cause| {
            relay_error(
                "internal_unexpected_failure",
                "failed to load startup failure history",
                Some(json!({
                    "namespace": enumerate_bundle.bundle_name,
                    "cause": cause,
                })),
            )
        })?;
    let startup_failure_count = recent_startup_failures.len();
    let configured_session_count = enumerate_bundle.members.len();
    let ready_session_count = sessions.iter().filter(|session| session.ready).count();
    let (state, startup_health, state_reason_code, state_reason) =
        list_bundle_state(configured_session_count, ready_session_count);
    let hosted = ready_session_count > 0;

    Ok(ListedBundle {
        id: enumerate_bundle.bundle_name.clone(),
        hosted,
        state,
        startup_health,
        state_reason_code,
        state_reason,
        startup_failure_count,
        recent_startup_failures,
        principals: sessions,
        principals_partial: None,
    })
}

/// Observational readiness for the `list` projection: it inspects a member's
/// transport and starts nothing.
///
/// Deliberately not the same function the bring-up passes use — `startup_member`
/// answers "bring this member up and tell me whether it is serving", this answers
/// "is it serving right now" — but deliberately the same *condition*. Both bottom
/// out in `resolve_active_pane_target` for tmux and in `acp_readiness_is_ready`
/// for ACP, because `degraded` is defined as one state across `up` and `list`: a
/// session one surface calls ready and the other calls failed makes the word
/// mean nothing.
fn session_ready_for_list(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
    tmux_socket: &Path,
    member: &crate::configuration::BundleMember,
) -> bool {
    match member.target {
        TargetConfiguration::Tmux(_) => {
            resolve_active_pane_target(tmux_socket, member.id.as_str()).is_ok()
        }
        TargetConfiguration::Acp(_) => acp_session_is_ready(
            bundle.bundle_name.as_str(),
            runtime_directory,
            member.id.as_str(),
        ),
        // Pty readiness mirrors the transport's readiness signal; the
        // bootstrap path (per-coder config parser) is responsible for
        // constructing the Pty transport. For v1 the relay treats Pty
        // readiness as a worker-side signal that the dispatcher reads
        // via the PtyOutputView handle; until that lands, we treat
        // configured Pty members as ready once the dispatcher has a
        // Pty transport wired up.
        TargetConfiguration::Pty(_) => true,
        // `ui`/`pubsub` members have no implemented startup path; they are
        // never counted ready and surface a startup failure on bundle up.
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => false,
    }
}

fn list_bundle_state(
    configured_session_count: usize,
    ready_session_count: usize,
) -> (
    ListedBundleState,
    Option<ListedBundleStartupHealth>,
    Option<String>,
    Option<String>,
) {
    if configured_session_count == 0 {
        (
            ListedBundleState::Down,
            None,
            Some("runtime_no_configured_sessions".to_string()),
            Some("bundle has zero configured sessions".to_string()),
        )
    } else if ready_session_count == 0 {
        (
            ListedBundleState::Down,
            None,
            Some("runtime_startup_failed".to_string()),
            Some("zero configured sessions are currently ready".to_string()),
        )
    } else {
        let health = if ready_session_count == configured_session_count {
            ListedBundleStartupHealth::Healthy
        } else {
            ListedBundleStartupHealth::Degraded
        };
        (ListedBundleState::Up, Some(health), None, None)
    }
}
