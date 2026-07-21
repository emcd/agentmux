//! Request dispatch: the per-bundle [`handle_request`] router and the relay-wide
//! entry points (`GLOBAL` list, identity admin/introspect, choices snapshot)
//! that bypass the per-bundle path. Each arm delegates to a sibling operation
//! submodule; this module owns only the routing and request normalization.

use std::path::Path;

use crate::configuration::BundleConfiguration;

use super::super::authorization::{AuthorizationContext, authorize_updown};
use super::super::identity::IdentityIntrospectRights;
use super::super::stream::{RelayStreamEvent, list_namespace_sessions};
use super::super::{
    BundleCatalog, ChoiceDecisionRequestContext, GLOBAL_NAMESPACE, ListedBundle, ListedBundleState,
    ListedSession, RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    bare_session_id, relay_error,
};
use super::{choices, identity, listing};

/// Per-bundle dispatcher for the operations whose subject is a bundle the
/// requester is a member of (`Up`/`Down`, `List`, choice decisions). The
/// target operations (`Send`/`Look`/`Raww`) are dispatched through their
/// namespace-centric paths and never reach here.
pub(in crate::relay) fn handle_request(
    request: RelayRequest,
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    let request = normalize_request_identities(request, bundle.bundle_name.as_str());
    match request {
        RelayRequest::Up => {
            authorize_bundle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_up(bundle, runtime_directory, bundle_catalog)
        }
        RelayRequest::Down => {
            authorize_bundle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_down(bundle, runtime_directory, bundle_catalog)
        }
        RelayRequest::List { requester_session } => listing::handle_list_routed(
            bundle,
            authorization,
            bundle,
            runtime_directory,
            requester_session,
        ),
        // Send, Look, and Raww are dispatched through the namespace-centric paths
        // (`handle_send_routed` / `handle_look_routed` / `handle_raww_routed`),
        // which resolve the requester in its home namespace rather than a borrowed
        // dispatch bundle. They never reach the per-bundle dispatcher.
        RelayRequest::Send { .. } | RelayRequest::Look { .. } | RelayRequest::Raww { .. } => {
            Err(relay_error(
                "internal_unexpected_request",
                "target operation reached the per-bundle dispatcher instead of its namespace-centric path",
                None,
            ))
        }
        RelayRequest::ChoicesPick {
            choice_request_id,
            outcome,
            option_id,
        } => choices::handle_choices_pick(
            bundle,
            authorization,
            ChoiceDecisionRequestContext {
                choice_request_id,
                outcome,
                option_id,
            },
            runtime_directory,
            principal,
        ),
        RelayRequest::ChoicesList => {
            choices::handle_choices_list(bundle, authorization, runtime_directory, principal)
        }
        RelayRequest::NewPeer { .. }
        | RelayRequest::ChangePsk { .. }
        | RelayRequest::IdentityIntrospect { .. }
        | RelayRequest::ListRelays
        | RelayRequest::DiscoverNamespaces { .. }
        | RelayRequest::DiscoverPrincipals { .. } => Err(relay_error(
            "internal_unexpected_request",
            "relay-wide request reached the per-bundle dispatcher",
            None,
        )),
    }
}

/// Returns every principal registered in namespace `GLOBAL` for a `List` request
/// issued with `namespace = "GLOBAL"`.
///
/// `GLOBAL` is a namespace in the unified registry like any bundle namespace, so
/// this enumerates every entry whose namespace is `GLOBAL` — including declared-
/// but-offline principals — rather than reading a dedicated relay-wide path. Each
/// principal's `ready` flag is computed at read time from the entry's connection
/// state, so an offline declared principal is listed with `ready = false` instead
/// of being omitted. The result is shaped as a `RelayResponse::List` over a
/// synthetic `GLOBAL` bundle view so clients reuse the existing list payload; the
/// bundle is `hosted`/`up` iff at least one principal is ready. An empty registry
/// yields an empty session set (a `down` bundle), not an error.
pub(in crate::relay) fn handle_global_list() -> RelayResponse {
    let mut sessions = list_namespace_sessions(GLOBAL_NAMESPACE)
        .into_iter()
        .map(|(principal_id, session_type, ready)| ListedSession {
            id: principal_id,
            name: None,
            transport: session_type.into(),
            ready,
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.id.cmp(&right.id));
    let hosted = sessions.iter().any(|session| session.ready);
    let state = if hosted {
        ListedBundleState::Up
    } else {
        ListedBundleState::Down
    };
    RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle: ListedBundle {
            id: "GLOBAL".to_string(),
            hosted,
            state,
            startup_health: None,
            state_reason_code: None,
            state_reason: None,
            startup_failure_count: 0,
            recent_startup_failures: Vec::new(),
            principals: sessions,
            principals_partial: None,
        },
    }
}

/// Dispatches a relay-wide identity administration request (`new peer`,
/// `change psk`). These bypass the per-bundle `handle_request` path: they
/// operate on the relay-level principal store and authorize against the
/// requester's policy preset relay-wide rather than within a bundle context.
pub(in crate::relay) fn handle_identity_admin_request(
    request: RelayRequest,
    configuration_root: &Path,
    state_root: &Path,
    requester_principal_id: &str,
) -> Result<RelayResponse, RelayError> {
    match request {
        RelayRequest::NewPeer {
            principal_id,
            scope,
            output_path,
        } => identity::handle_new_peer(
            configuration_root,
            state_root,
            requester_principal_id,
            identity::NewPeerRequestContext {
                principal_id,
                scope,
                output_path,
            },
        ),
        RelayRequest::ChangePsk { principal_id } => identity::handle_change_psk(
            configuration_root,
            state_root,
            requester_principal_id,
            principal_id,
        ),
        _ => Err(relay_error(
            "internal_unexpected_request",
            "non-admin request routed to identity admin dispatcher",
            None,
        )),
    }
}

/// Dispatches a relay-wide `IdentityIntrospect` request. Like the identity admin
/// handlers this bypasses the per-bundle `handle_request` path: introspection
/// reads the relay-level principal store and its target may be a bundle-less
/// principal. The connection gate (the recorded `introspect_rights` and their
/// scope) is enforced inside the handler.
pub(in crate::relay) fn handle_identity_introspect(
    state_root: &Path,
    principal: &RequestPrincipal,
    target_session: &str,
) -> Result<RelayResponse, RelayError> {
    identity::handle_identity_introspect(state_root, principal, target_session)
}

/// Builds the `identity.snapshot` stream event for a trusted-host connection's
/// registered scope (delivered once, right after Hello). See
/// `identity::build_identity_snapshot_event`.
pub(in crate::relay) fn build_identity_snapshot_event(
    state_root: &Path,
    host_principal_id: &str,
    rights: &IdentityIntrospectRights,
) -> Result<RelayStreamEvent, RelayError> {
    identity::build_identity_snapshot_event(state_root, host_principal_id, rights)
}

pub(in crate::relay) fn emit_choices_snapshot_for_ui_registration(
    configuration_root: &Path,
    namespace: &str,
    runtime_directory: &Path,
    ui_session_id: &str,
) -> Result<(), RelayError> {
    choices::emit_choices_snapshot_for_ui_registration(
        configuration_root,
        namespace,
        runtime_directory,
        ui_session_id,
    )
}

/// Rewrites canonical `session@bundle` identities in an incoming request to
/// their bundle-local form so internal lookups match configured member ids.
fn normalize_request_identities(request: RelayRequest, namespace: &str) -> RelayRequest {
    let bare = |id: String| bare_session_id(id.as_str(), namespace);
    match request {
        RelayRequest::List { requester_session } => RelayRequest::List {
            requester_session: requester_session.map(bare),
        },
        RelayRequest::Send {
            request_id,
            requester_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            on_behalf_of,
        } => RelayRequest::Send {
            request_id,
            requester_session: bare(requester_session),
            message,
            // Targets stay fully qualified; the relay no longer bares them. The
            // send path classifies each target from its `@<namespace>` suffix.
            targets,
            broadcast,
            quiet_window_ms,
            // A peer-forwarded origin attribution survives normalization unchanged.
            on_behalf_of,
        },
        RelayRequest::Look {
            requester_session,
            target_session,
            lines,
            offset,
        } => RelayRequest::Look {
            requester_session: bare(requester_session),
            target_session,
            lines,
            offset,
        },
        RelayRequest::Raww {
            request_id,
            requester_session,
            target_session,
            text,
            no_enter,
            on_behalf_of,
        } => RelayRequest::Raww {
            request_id,
            requester_session: bare(requester_session),
            target_session,
            text,
            no_enter,
            on_behalf_of,
        },
        request @ (RelayRequest::Up
        | RelayRequest::Down
        | RelayRequest::ChoicesPick { .. }
        | RelayRequest::ChoicesList
        | RelayRequest::NewPeer { .. }
        | RelayRequest::ChangePsk { .. }
        | RelayRequest::IdentityIntrospect { .. }
        | RelayRequest::ListRelays
        | RelayRequest::DiscoverNamespaces { .. }
        | RelayRequest::DiscoverPrincipals { .. }) => request,
    }
}

fn authorize_bundle_principal(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    principal: Option<&RequestPrincipal>,
) -> Result<(), RelayError> {
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "bundle up/down requests require stream-associated principal identity",
            None,
        )
    })?;
    authorize_updown(bundle, authorization, principal.session_id.as_str())
}
