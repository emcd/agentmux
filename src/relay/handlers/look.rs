use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    acp::state::ACP_LOOK_PRIME_TIMEOUT_MS,
    configuration::BundleConfiguration,
    relay::{AcpLookFreshness, AcpLookSnapshotSource, LookSnapshotPayload},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
    transports::{LookMode, LookSnapshotPayload as TransportLookSnapshotPayload},
};

use super::super::connection::BundleCatalog;
use super::super::delivery::get_acp_worker_output_view;
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
use crate::tmux::pane::{capture_pane_tail_lines, resolve_active_pane_target};

const LOOK_LINES_DEFAULT: usize = 120;
const LOOK_LINES_MAX: usize = 1000;
/// Default ACP look window. ACP replay entries are far larger than tmux lines
/// (each can be a full message or tool invocation), so a small default keeps
/// the response under the MCP payload limit while still showing recent context.
const ACP_LOOK_ENTRIES_DEFAULT: usize = 50;

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

/// Captures the look snapshot for the authorized target and builds the response.
/// The target is guaranteed present by `prepare_look`.
/// Translates the transport-layer look snapshot into the relay's wire payload.
/// The freshness and source enums are shared types (the transport contract
/// reuses the relay's), so only the variant shape differs.
fn transport_acp_snapshot_to_wire(snapshot: TransportLookSnapshotPayload) -> LookSnapshotPayload {
    match snapshot {
        TransportLookSnapshotPayload::AcpEntries {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness,
            snapshot_source,
            stale_reason_code,
            snapshot_age_ms,
        } => LookSnapshotPayload::AcpEntriesV1 {
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

    let snapshot = match &target.target {
        crate::configuration::TargetConfiguration::Tmux(_) => {
            if offset != 0 {
                return Err(relay_error(
                    "validation_offset_unsupported",
                    "offset is only supported for ACP look targets",
                    Some(json!({
                        "target_session": target.id,
                        "offset": offset,
                    })),
                ));
            }
            let requested_lines = lines.unwrap_or(LOOK_LINES_DEFAULT);
            let tmux_socket =
                tmux_socket_path_for_runtime_directory(look_runtime_directory.as_path());
            let pane_target = resolve_active_pane_target(tmux_socket.as_path(), target.id.as_str())
                .map_err(|reason| {
                    relay_error(
                        "internal_unexpected_failure",
                        "failed to resolve active pane for look target",
                        Some(json!({"target_session": target.id, "cause": reason})),
                    )
                })?;
            let snapshot_lines = capture_pane_tail_lines(
                tmux_socket.as_path(),
                pane_target.as_str(),
                requested_lines,
            )
            .map_err(|reason| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to capture look snapshot",
                    Some(json!({"target_session": target.id, "cause": reason})),
                )
            })?;
            LookSnapshotPayload::Lines { snapshot_lines }
        }
        crate::configuration::TargetConfiguration::Ui
        | crate::configuration::TargetConfiguration::Pubsub => {
            unreachable!("capability gate in prepare_look rejects can_be_looked = false targets")
        }
        crate::configuration::TargetConfiguration::Acp(_) => {
            let requested_entries = lines.unwrap_or(ACP_LOOK_ENTRIES_DEFAULT);
            // The look path reads the OutputView handle published by the ACP
            // transport (which owns the bounded prime-wait + freshness). A
            // missing handle means the worker is unstarted, failed bootstrap, or
            // mid-respawn: surface an empty, stale/unavailable snapshot rather
            // than reading any buffer.
            match get_acp_worker_output_view(
                look_bundle.bundle_name.as_str(),
                look_runtime_directory.as_path(),
                target.id.as_str(),
            ) {
                Some(view) => {
                    let mode = LookMode {
                        lines: Some(requested_entries as u64),
                        offset: Some(offset as u64),
                        prime_timeout: Duration::from_millis(ACP_LOOK_PRIME_TIMEOUT_MS),
                    };
                    let snapshot = view.look(mode).map_err(|error| {
                        relay_error(
                            "internal_unexpected_failure",
                            "failed to capture ACP look snapshot",
                            Some(json!({
                                "target_session": target.id,
                                "code": error.code,
                                "cause": error.reason,
                            })),
                        )
                    })?;
                    transport_acp_snapshot_to_wire(snapshot)
                }
                None => LookSnapshotPayload::AcpEntriesV1 {
                    snapshot_entries: Vec::new(),
                    entries_total: 0,
                    returned_entries_count: 0,
                    freshness: AcpLookFreshness::Stale,
                    snapshot_source: AcpLookSnapshotSource::None,
                    stale_reason_code: Some("acp_worker_unavailable".to_string()),
                    snapshot_age_ms: None,
                },
            }
        }
    };
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
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries,
                entries_total,
                freshness,
                snapshot_source,
                stale_reason_code,
                snapshot_age_ms,
                ..
            } => (
                "acp_entries_v1",
                snapshot_entries.len(),
                Some(*entries_total),
                Some(match freshness {
                    AcpLookFreshness::Fresh => "fresh",
                    AcpLookFreshness::Stale => "stale",
                }),
                Some(match snapshot_source {
                    AcpLookSnapshotSource::LiveBuffer => "live_buffer",
                    AcpLookSnapshotSource::None => "none",
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
