use std::path::Path;

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::{BundleConfiguration, load_bundle_configuration},
    relay::{AcpLookFreshness, AcpLookSnapshotSource, LookSnapshotPayload},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
};

use super::super::authorization::{AuthorizationContext, authorize_route};
use super::super::connection::BundleCatalog;
use super::super::delivery::{
    await_acp_worker_prime_for_look, derive_acp_look_snapshot, get_acp_worker_snapshot,
    get_acp_worker_state,
};
use super::super::routing::{
    Addressing, Capability, OperationProfile, requester_home_namespace, resolve_look_route,
};
use super::super::tmux::{capture_pane_tail_lines, resolve_active_pane_target};
use super::super::{
    LookRequestContext, RelayError, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    canonical_session_id, map_config, relay_error, session_type_not_implemented,
};
use super::sender::resolve_sender_identity;

const LOOK_LINES_DEFAULT: usize = 120;
const LOOK_LINES_MAX: usize = 1000;
/// Default ACP look window. ACP replay entries are far larger than tmux lines
/// (each can be a full message or tool invocation), so a small default keeps
/// the response under the MCP payload limit while still showing recent context.
const ACP_LOOK_ENTRIES_DEFAULT: usize = 50;

pub(super) fn handle_look(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: LookRequestContext,
    runtime_directory: &Path,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let LookRequestContext {
        requester_session,
        target_session,
        lines,
        offset,
    } = request;

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

    // The requester is always a member of the dispatch (connection) bundle; a
    // session is only ever a member of its home bundle.
    let requester = resolve_sender_identity(
        bundle,
        authorization,
        requester_session.as_str(),
        "requester_session",
    )?;

    // Resolve the target through the config-free single-target stage: a bare
    // (unqualified) target is rejected, a reserved namespace names no pane, and
    // an `@<bundle>` target is classified by suffix alone. The route doubles as
    // the authorization input below.
    let route = resolve_look_route(
        requester_home_namespace(requester.session_id.as_str(), bundle.bundle_name.as_str()),
        requester.session_id.as_str(),
        target_session.as_str(),
    )?;
    let target_route = &route.targets[0];
    let target_bundle_name = target_route.bundle_name.as_str();
    let target_session_id = target_route
        .session_id
        .as_deref()
        .expect("look target carries a session id");
    let cross_bundle = target_bundle_name != bundle.bundle_name;

    // For a peer-bundle target, load its configuration and runtime context from
    // the catalog; same-bundle looks reuse the dispatch bundle as-is.
    let peer_bundle;
    let (look_bundle, look_runtime_directory) = if cross_bundle {
        let Some(paths) = bundle_catalog.lookup(target_bundle_name) else {
            return Err(relay_error(
                "validation_unknown_bundle",
                "look target bundle is not configured on this relay",
                Some(json!({ "bundle_name": target_bundle_name })),
            ));
        };
        peer_bundle = load_bundle_configuration(configuration_root, target_bundle_name)
            .map_err(map_config)?;
        (&peer_bundle, paths.runtime_directory.clone())
    } else {
        (bundle, runtime_directory.to_path_buf())
    };

    let target = look_bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not in bundle configuration",
                Some(json!({
                    "target_session": canonical_session_id(target_session_id, target_bundle_name),
                })),
            )
        })?;

    // Authorize through the uniform routing/authorization spine: the requester's
    // `look` control is resolved in its dispatch bundle, and a peer-bundle target
    // raises the required tier to `all:all` while same-bundle inspection of
    // another session needs `all:home` (self-inspection needs only `self`).
    // Existence is validated above, so a denial sorts after `validation_unknown_target`.
    authorize_route(
        bundle.bundle_name.as_str(),
        authorization,
        OperationProfile {
            capability: Capability::Look,
            addressing: Addressing::SingleTarget,
        },
        &route,
    )?;

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
            return Err(session_type_not_implemented(
                target.id.as_str(),
                target.target.session_type(),
            ));
        }
        crate::configuration::TargetConfiguration::Acp(_) => {
            let prime_timed_out = await_acp_worker_prime_for_look(
                look_bundle,
                target,
                look_runtime_directory.as_path(),
            )
            .map_err(|reason| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to await ACP worker prime for look",
                    Some(json!({"target_session": target.id, "cause": reason})),
                )
            })?;
            let worker_state = get_acp_worker_state(
                look_bundle.bundle_name.as_str(),
                look_runtime_directory.as_path(),
                target.id.as_str(),
            );
            let worker_snapshot = get_acp_worker_snapshot(
                look_bundle.bundle_name.as_str(),
                look_runtime_directory.as_path(),
                target.id.as_str(),
            );
            let requested_entries = lines.unwrap_or(ACP_LOOK_ENTRIES_DEFAULT);
            let snapshot = derive_acp_look_snapshot(
                worker_state,
                worker_snapshot.as_deref().map(Vec::as_slice),
                requested_entries,
                offset,
                prime_timed_out,
            );
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries: snapshot.snapshot_entries,
                entries_total: snapshot.entries_total,
                returned_entries_count: snapshot.returned_entries_count,
                freshness: snapshot.freshness,
                snapshot_source: snapshot.snapshot_source,
                stale_reason_code: snapshot.stale_reason_code,
                snapshot_age_ms: snapshot.snapshot_age_ms,
            }
        }
    };
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: look_bundle.bundle_name.clone(),
        requester_session: canonical_session_id(
            requester.session_id.as_str(),
            bundle.bundle_name.as_str(),
        ),
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
        bundle_name,
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
                "bundle_name": bundle_name,
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
