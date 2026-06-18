use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    acp::state::ACP_LOOK_PRIME_TIMEOUT_MS,
    configuration::BundleConfiguration,
    relay::{LookFreshness, LookSnapshotPayload, LookSnapshotSource},
    runtime::inscriptions::emit_inscription,
    transports::{LookMode, LookSnapshotPayload as TransportLookSnapshotPayload, TransportError},
};

use super::super::connection::BundleCatalog;
use super::super::delivery::get_output_view;
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, requester_home_namespace,
    resolve_look_route,
};
use super::super::{
    RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION, bare_session_id,
    canonical_session_id, relay_error, unsupported_operation,
};
use super::routed::{
    load_home_context, relay_wide_operation_unimplemented, resolve_relay_wide_target_session_type,
    resolve_target_bundle, run_target_operation,
};
use super::sender::resolve_sender_in_namespace;

/// Shared upper bound on the caller-supplied look window, validated before
/// dispatch. The per-transport *default* window lives in each transport's
/// `OutputView::look`.
const LOOK_LINES_MAX: usize = 1000;

/// The look target's bundle and runtime, resolved and existence-validated by
/// `prepare_look` before authorization.
struct LookPrepared {
    bundle: BundleConfiguration,
    runtime_directory: PathBuf,
}

/// Entry point for the namespace-centric look path. The requester is resolved and
/// authorized in its **home** namespace (`home_namespace`: its bound bundle, or
/// `GLOBAL`); the target's bundle is loaded separately for the snapshot. See
/// `dispatch_look`.
pub(in crate::relay) fn handle_look_routed(
    home_namespace: &str,
    home_runtime_directory: Option<&Path>,
    request: RelayRequest,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Look {
        requester_session,
        target_session,
        lines,
        offset,
    } = request
    else {
        return Err(relay_error(
            "internal_unexpected_request",
            "non-look request routed to the look dispatcher",
            None,
        ));
    };

    // Validate the caller-supplied window size against the shared bound; the
    // transport-specific default is applied per branch below, since ACP entries
    // and tmux lines have very different size profiles.
    if let Some(requested_lines) = lines
        && !(1..=LOOK_LINES_MAX).contains(&requested_lines)
    {
        return Err(relay_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": requested_lines,
                "min": 1,
                "max": LOOK_LINES_MAX,
            })),
        ));
    }
    let offset = offset.unwrap_or(0);

    // The requester is identified and authorized in its home namespace (operator
    // policy for `GLOBAL`, or the bundle's policy), never a borrowed target bundle.
    let (home_bundle, authorization) = load_home_context(home_namespace, configuration_root)?;
    let requester_session = bare_session_id(requester_session.as_str(), home_namespace);
    let requester = resolve_sender_in_namespace(
        home_bundle.as_ref(),
        &authorization,
        requester_session.as_str(),
        "requester_session",
    )?;

    // The spine owns resolution and authorization; `prepare_look` validates
    // existence and loads the target bundle, `execute_look` captures the snapshot.
    run_target_operation(
        home_namespace,
        &authorization,
        OperationProfile {
            capability: Capability::Look,
            addressing: Addressing::SingleTarget,
        },
        || {
            resolve_look_route(
                requester_home_namespace(requester.session_id.as_str(), home_namespace),
                requester.session_id.as_str(),
                target_session.as_str(),
            )
        },
        |route| {
            prepare_look(
                route,
                home_namespace,
                home_bundle.as_ref(),
                home_runtime_directory,
                configuration_root,
                bundle_catalog,
            )
        },
        |route, prepared| {
            execute_look(
                route,
                prepared,
                requester.session_id.as_str(),
                home_namespace,
                lines,
                offset,
                principal,
            )
        },
    )
}

/// Resolves the look target's bundle, validates that the target session is a
/// configured member, and applies the `can_be_looked` capability gate. Loads
/// peer configuration for a cross-namespace target; reuses the home bundle for
/// a same-namespace one. Runs before authorization, so an unknown or
/// transport-incapable target sorts before `authorization_forbidden`.
fn prepare_look(
    route: &ResolvedRoute,
    home_namespace: &str,
    home_bundle: Option<&BundleConfiguration>,
    home_runtime_directory: Option<&Path>,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<LookPrepared, RelayError> {
    let target_route = &route.targets[0];
    let target_bundle_name = target_route.bundle_name.as_str();
    let target_session_id = target_route
        .session_id
        .as_deref()
        .expect("look target carries a session id");
    if target_route.relay_wide {
        // A relay-wide target has no bundle membership; its capability derives
        // from the declared session type in the global users configuration.
        let session_type =
            resolve_relay_wide_target_session_type(configuration_root, target_session_id)?;
        if !session_type.can_be_looked() {
            return Err(unsupported_operation(
                target_session_id,
                session_type,
                "can_be_looked",
            ));
        }
        return Err(relay_wide_operation_unimplemented(
            "look",
            target_session_id,
            session_type,
        ));
    }
    let (bundle, runtime_directory) = resolve_target_bundle(
        home_namespace,
        home_bundle,
        home_runtime_directory,
        target_bundle_name,
        configuration_root,
        bundle_catalog,
    )?;
    let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
    else {
        return Err(relay_error(
            "validation_unknown_target",
            "target_session is not in bundle configuration",
            Some(json!({
                "target_session": canonical_session_id(target_session_id, target_bundle_name),
            })),
        ));
    };
    let session_type = member.target.session_type();
    if !session_type.can_be_looked() {
        return Err(unsupported_operation(
            canonical_session_id(target_session_id, target_bundle_name).as_str(),
            session_type,
            "can_be_looked",
        ));
    }
    Ok(LookPrepared {
        bundle,
        runtime_directory,
    })
}

/// Translates the transport-layer look snapshot into the relay's wire payload.
/// The freshness and source enums are shared types (the relay re-exports the
/// transport vocabulary), so only the variant discriminator differs.
fn transport_snapshot_to_wire(snapshot: TransportLookSnapshotPayload) -> LookSnapshotPayload {
    match snapshot {
        TransportLookSnapshotPayload::StructuredEntries {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness,
            snapshot_source,
            stale_reason_code,
            snapshot_age_ms,
        } => LookSnapshotPayload::StructuredEntriesV1 {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness,
            snapshot_source,
            stale_reason_code,
            snapshot_age_ms,
        },
        TransportLookSnapshotPayload::Lines { snapshot_lines } => {
            LookSnapshotPayload::Lines { snapshot_lines }
        }
    }
}

/// The empty stale snapshot returned when an ACP look target has no published
/// output-view handle (worker unstarted, failed bootstrap, or mid-respawn).
/// Returned uniformly through the wire translation so the handler shapes no
/// payload by transport identity.
fn unavailable_acp_look_snapshot() -> TransportLookSnapshotPayload {
    TransportLookSnapshotPayload::StructuredEntries {
        snapshot_entries: Vec::new(),
        entries_total: 0,
        returned_entries_count: 0,
        freshness: LookFreshness::Stale,
        snapshot_source: LookSnapshotSource::None,
        stale_reason_code: Some("acp_worker_unavailable".to_string()),
        snapshot_age_ms: None,
    }
}

/// Maps an `OutputView::look` failure onto a relay error. Validation-class
/// transport codes (e.g. `validation_offset_unsupported`) become relay
/// validation errors with their code preserved; anything else is an internal
/// failure.
fn map_look_transport_error(target_session: &str, error: TransportError) -> RelayError {
    // Carry the transport's own `details` through so the underlying cause (e.g.
    // the tmux capture failure reason) is not dropped on the relay boundary.
    let details = Some(json!({
        "target_session": target_session,
        "code": error.code,
        "details": error.details,
    }));
    if error.code.starts_with("validation_") {
        relay_error(error.code.as_str(), error.reason.as_str(), details)
    } else {
        relay_error(
            "internal_unexpected_failure",
            error.reason.as_str(),
            details,
        )
    }
}

fn execute_look(
    route: &ResolvedRoute,
    prepared: LookPrepared,
    requester_session_id: &str,
    home_namespace: &str,
    lines: Option<usize>,
    offset: usize,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let LookPrepared {
        bundle: look_bundle,
        runtime_directory: look_runtime_directory,
    } = prepared;
    let target_session_id = route.targets[0]
        .session_id
        .as_deref()
        .expect("look target carries a session id");
    let target = look_bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
        .expect("look target existence validated in prepare_look");

    // One polymorphic look call: the accessor resolves the per-transport handle
    // provenance (worker-published for ACP, config-constructed for tmux), the
    // handle owns its own windowing default, `LookMode` validation, and (for
    // ACP) the bounded prime-wait + freshness. A `None` handle is the ACP
    // missing-worker case, surfaced as an empty stale/unavailable snapshot.
    let mode = LookMode {
        lines: lines.map(|lines| lines as u64),
        offset: Some(offset as u64),
        prime_timeout: Duration::from_millis(ACP_LOOK_PRIME_TIMEOUT_MS),
    };
    let snapshot = match get_output_view(
        look_bundle.bundle_name.as_str(),
        look_runtime_directory.as_path(),
        target,
    ) {
        Some(view) => view
            .look(mode)
            .map_err(|error| map_look_transport_error(target.id.as_str(), error))?,
        None => unavailable_acp_look_snapshot(),
    };
    let snapshot = transport_snapshot_to_wire(snapshot);
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        requester_session: canonical_session_id(requester_session_id, home_namespace),
        target_session: canonical_session_id(target.id.as_str(), look_bundle.bundle_name.as_str()),
        captured_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        authenticated_identity: principal
            .and_then(|principal| principal.authenticated_identity.clone()),
        on_behalf_of: None,
        snapshot,
    };
    if let RelayResponse::Look {
        requester_session,
        target_session,
        snapshot,
        ..
    } = &response
    {
        let (
            snapshot_format,
            snapshot_count,
            entries_total,
            freshness_label,
            snapshot_source_label,
            stale_reason_code,
            snapshot_age_ms,
        ) = match snapshot {
            LookSnapshotPayload::Lines { snapshot_lines } => {
                ("lines", snapshot_lines.len(), None, None, None, None, None)
            }
            LookSnapshotPayload::StructuredEntriesV1 {
                snapshot_entries,
                entries_total,
                freshness,
                snapshot_source,
                stale_reason_code,
                snapshot_age_ms,
                ..
            } => (
                "structured_entries_v1",
                snapshot_entries.len(),
                Some(*entries_total),
                Some(match freshness {
                    LookFreshness::Fresh => "fresh",
                    LookFreshness::Stale => "stale",
                }),
                Some(match snapshot_source {
                    LookSnapshotSource::LiveBuffer => "live_buffer",
                    LookSnapshotSource::None => "none",
                }),
                stale_reason_code.as_deref(),
                *snapshot_age_ms,
            ),
        };
        emit_inscription(
            "relay.look.response",
            &json!({
                "bundle_name": look_bundle.bundle_name,
                "requester_session": requester_session,
                "target_session": target_session,
                "snapshot_format": snapshot_format,
                "snapshot_count": snapshot_count,
                "entries_total": entries_total,
                "lines_requested": lines,
                "offset": offset,
                "freshness": freshness_label,
                "snapshot_source": snapshot_source_label,
                "stale_reason_code": stale_reason_code,
                "snapshot_age_ms": snapshot_age_ms,
            }),
        );
    }
    Ok(response)
}
