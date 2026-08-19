//! The Request frame: gating it against the connection's registration, then
//! routing it to the dispatcher its operation belongs to.
//!
//! Routing here is about *which bundle context an operation runs in*, not about
//! what the operation does. Several families deliberately bypass bundle routing
//! — identity administration and introspection are relay-level, relay-wide
//! `List` and discovery read the registry and configured peers, and
//! `Send`/`Look`/`Raww` authorize in the requester's home namespace and load
//! each target's bundle inside the handler. What remains is bundle-subject.

use std::{io, sync::Arc};

use serde_json::json;

use super::super::stream::{OutgoingFrame, registration_is_current, write_stream_frame_to_writer};
use super::super::{
    RelayRequest, RelayResponse, RequestPrincipal, dispatch_discovery, dispatch_identity_admin,
    dispatch_identity_introspect, dispatch_list, dispatch_look, dispatch_raww, dispatch_request,
    dispatch_send, handlers, relay_error,
};
use super::framing::dispatch_on_blocking_pool;
use super::helpers::{full_requester_principal_id, resolve_namespace_routing_bundle};
use super::serve::{ConnectionBinding, FrameContext, FrameOutcome, RegistrationGuard};

/// Routes one request frame and writes its response.
pub(super) async fn handle_request(
    request_id: Option<String>,
    target_namespace: Option<String>,
    request: RelayRequest,
    frame: &FrameContext<'_>,
    guard: &RegistrationGuard,
    binding: &ConnectionBinding,
) -> Result<FrameOutcome, io::Error> {
    let writer = frame.writer;
    let bundle_catalog = &frame.context.bundle_catalog;
    let peer_connection_manager = &frame.context.peer_connection_manager;
    let Some(active_registration) = guard.current() else {
        let error = relay_error(
            "validation_missing_hello",
            "stream request requires hello registration",
            None,
        );
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &RelayResponse::Error { error },
            },
        )?;
        return Ok(FrameOutcome::Next);
    };
    if !registration_is_current(active_registration)? {
        let error = relay_error(
            "validation_stale_stream_binding",
            "stream binding has been replaced by a newer hello registration",
            Some(json!({
                "principal_id": active_registration.requester_session_id(),
                "namespace": active_registration.namespace(),
            })),
        );
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &RelayResponse::Error { error },
            },
        )?;
        return Ok(FrameOutcome::Stop);
    }
    // Relay-wide identity administration bypasses bundle routing:
    // it mutates the relay-level principal store and authorizes the
    // operator against their policy preset relay-wide.
    if matches!(
        request,
        RelayRequest::NewPeer { .. } | RelayRequest::ChangePsk { .. }
    ) {
        let requester_principal_id = full_requester_principal_id(active_registration);
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            let state_root = Arc::clone(frame.state_root);
            let identity_admin_lock = Arc::clone(&frame.context.identity_admin_lock);
            dispatch_on_blocking_pool(move || {
                dispatch_identity_admin(
                    request,
                    &configuration_roots,
                    &state_root,
                    requester_principal_id.as_str(),
                    &identity_admin_lock,
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // Identity introspection is relay-wide: it reads the relay-level
    // principal store and its target may be a bundle-less principal,
    // so it bypasses per-bundle routing. The gate is the connection's
    // recorded `introspect_rights`, carried on the request principal.
    if matches!(request, RelayRequest::IdentityIntrospect { .. }) {
        let principal = RequestPrincipal {
            session_id: active_registration.requester_session_id().to_string(),
            authenticated_identity: binding.authenticated_identity.clone(),
            introspect_rights: binding.introspect_rights.clone(),
            ingress_scope: binding.ingress_scope.clone(),
        };
        let response = {
            let state_root = Arc::clone(frame.state_root);
            dispatch_on_blocking_pool(move || {
                dispatch_identity_introspect(request, &state_root, &principal)
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // `List` with the `GLOBAL` namespace enumerates relay-wide
    // sessions, which have no bundle context; it bypasses the
    // per-bundle dispatch path and reads the stream registry
    // directly. Other per-target operations infer their routing
    // bundle from target suffixes inside `resolve_effective_bundle`.
    if matches!(request, RelayRequest::List { .. }) && target_namespace.as_deref() == Some("GLOBAL")
    {
        let response = handlers::handle_global_list();
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // Every other `List` routes its home (dispatch) bundle separately
    // from the enumerated bundle, so a session can list a peer bundle
    // without being looked up in the wrong bundle's members. The
    // enumerated bundle is the wire `namespace` (or the bound bundle);
    // the dispatch bundle is the requester's bound bundle, or — for a
    // relay-wide principal with no home bundle — the enumerated bundle,
    // where its TUI-config controls resolve (preserving relay-wide list
    // reach).
    if matches!(request, RelayRequest::List { .. }) {
        let enumerate_paths = match resolve_namespace_routing_bundle(
            bundle_catalog,
            target_namespace.as_deref(),
            binding.bound_bundle.as_ref(),
        ) {
            Ok(paths) => paths,
            Err(error) => {
                write_stream_frame_to_writer(
                    writer,
                    OutgoingFrame::Response {
                        request_id: request_id.as_deref(),
                        response: &RelayResponse::Error { error },
                    },
                )?;
                return Ok(FrameOutcome::Next);
            }
        };
        let dispatch_paths = binding
            .bound_bundle
            .clone()
            .unwrap_or_else(|| enumerate_paths.clone());
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            dispatch_on_blocking_pool(move || {
                dispatch_list(
                    request,
                    &configuration_roots,
                    &dispatch_paths,
                    &enumerate_paths,
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // `Send`, `Look`, and `Raww` route per-target by namespace and are
    // authorized in the requester's home namespace (its bound bundle,
    // or `GLOBAL`), never a borrowed peer/target bundle — so they
    // bypass the bundle-subject `resolve_namespace_routing_bundle`
    // path below and load each target's bundle inside the handler.
    if matches!(request, RelayRequest::Send { .. }) {
        let principal = RequestPrincipal {
            session_id: active_registration.requester_session_id().to_string(),
            authenticated_identity: binding.authenticated_identity.clone(),
            introspect_rights: binding.introspect_rights.clone(),
            ingress_scope: binding.ingress_scope.clone(),
        };
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            let bound_bundle = binding.bound_bundle.clone();
            let bundle_catalog = bundle_catalog.clone();
            let peer_connection_manager = Arc::clone(peer_connection_manager);
            dispatch_on_blocking_pool(move || {
                dispatch_send(
                    request,
                    &configuration_roots,
                    bound_bundle.as_ref(),
                    Some(principal),
                    &bundle_catalog,
                    peer_connection_manager.as_ref(),
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    if matches!(request, RelayRequest::Look { .. }) {
        let principal = RequestPrincipal {
            session_id: active_registration.requester_session_id().to_string(),
            authenticated_identity: binding.authenticated_identity.clone(),
            introspect_rights: binding.introspect_rights.clone(),
            ingress_scope: binding.ingress_scope.clone(),
        };
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            let bound_bundle = binding.bound_bundle.clone();
            let bundle_catalog = bundle_catalog.clone();
            dispatch_on_blocking_pool(move || {
                dispatch_look(
                    request,
                    &configuration_roots,
                    bound_bundle.as_ref(),
                    Some(principal),
                    &bundle_catalog,
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    if matches!(request, RelayRequest::Raww { .. }) {
        let principal = RequestPrincipal {
            session_id: active_registration.requester_session_id().to_string(),
            authenticated_identity: binding.authenticated_identity.clone(),
            introspect_rights: binding.introspect_rights.clone(),
            ingress_scope: binding.ingress_scope.clone(),
        };
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            let bound_bundle = binding.bound_bundle.clone();
            let bundle_catalog = bundle_catalog.clone();
            let peer_connection_manager = Arc::clone(peer_connection_manager);
            dispatch_on_blocking_pool(move || {
                dispatch_raww(
                    request,
                    &configuration_roots,
                    bound_bundle.as_ref(),
                    Some(principal),
                    &bundle_catalog,
                    peer_connection_manager.as_ref(),
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // Relay-wide discovery (`list.relays`, `list.namespaces`,
    // cross-relay `list.principals`) is not a bundle-subject
    // operation: it reads the configured peer aliases and this relay's
    // own catalog/registry and forwards foreign discovery through the
    // peer connection manager. The requester is its authenticated
    // principal, so it bypasses the bundle-routing path below.
    if matches!(
        request,
        RelayRequest::ListRelays
            | RelayRequest::DiscoverNamespaces { .. }
            | RelayRequest::DiscoverPrincipals { .. }
    ) {
        // Discovery authorization resolves the requester's controls
        // relay-wide, so it needs the full canonical `<id>@<namespace>`
        // principal id — a bundle session's stored id is bundle-local.
        let principal = RequestPrincipal {
            session_id: full_requester_principal_id(active_registration),
            authenticated_identity: binding.authenticated_identity.clone(),
            introspect_rights: binding.introspect_rights.clone(),
            ingress_scope: binding.ingress_scope.clone(),
        };
        let response = {
            let configuration_roots = Arc::clone(frame.configuration_roots);
            let bundle_catalog = bundle_catalog.clone();
            let peer_connection_manager = Arc::clone(peer_connection_manager);
            let relay_aliases = Arc::clone(&frame.context.relay_aliases);
            dispatch_on_blocking_pool(move || {
                dispatch_discovery(
                    request,
                    &configuration_roots,
                    principal,
                    &bundle_catalog,
                    peer_connection_manager.as_ref(),
                    relay_aliases.as_slice(),
                )
            })
            .await
        };
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: request_id.as_deref(),
                response: &response,
            },
        )?;
        return Ok(FrameOutcome::Next);
    }
    // Bundle-subject operations (`Up`/`Down`, choice decisions)
    // address a bundle the requester is a member of, by the wire
    // `namespace` selector or the bound bundle. This is not a borrow:
    // the bundle is the operation's subject, not a stand-in home.
    let bundle_paths = match resolve_namespace_routing_bundle(
        bundle_catalog,
        target_namespace.as_deref(),
        binding.bound_bundle.as_ref(),
    ) {
        Ok(bundle_paths) => bundle_paths,
        Err(error) => {
            write_stream_frame_to_writer(
                writer,
                OutgoingFrame::Response {
                    request_id: request_id.as_deref(),
                    response: &RelayResponse::Error { error },
                },
            )?;
            return Ok(FrameOutcome::Next);
        }
    };
    let principal = RequestPrincipal {
        session_id: active_registration.requester_session_id().to_string(),
        authenticated_identity: binding.authenticated_identity.clone(),
        introspect_rights: binding.introspect_rights.clone(),
        ingress_scope: binding.ingress_scope.clone(),
    };
    let response = {
        let configuration_roots = Arc::clone(frame.configuration_roots);
        let bundle_catalog = bundle_catalog.clone();
        dispatch_on_blocking_pool(move || {
            dispatch_request(
                request,
                &configuration_roots,
                &bundle_paths.bundle_name,
                &bundle_paths.runtime_directory,
                Some(principal),
                &bundle_catalog,
            )
        })
        .await
    };
    write_stream_frame_to_writer(
        writer,
        OutgoingFrame::Response {
            request_id: request_id.as_deref(),
            response: &response,
        },
    )?;
    Ok(FrameOutcome::Next)
}
