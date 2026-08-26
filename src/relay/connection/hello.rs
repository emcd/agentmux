//! The Hello frame: verifying an identity, claiming the registry entry for it,
//! and recording what the rest of the connection is authorized to do.
//!
//! Everything a later request frame reads about who is connected is established
//! here, in [`ConnectionBinding`]. A Hello that cannot be verified, cannot claim
//! its identity, or cannot be given its post-registration snapshots ends the
//! connection rather than leaving it half-bound.

use std::io;

use crate::configuration::SessionType;

use super::super::stream::{
    HelloFrame, OutgoingFrame, RegisterStreamOutcome, StreamRevokeSignal, register_stream,
    write_stream_frame_to_writer,
};
use super::super::{IdentityIntrospectRights, RelayResponse, SCHEMA_VERSION, handlers};
use super::helpers::{
    emit_registration_choices_snapshots, identity_claim_conflict_error, resolve_hello_binding,
};
use super::serve::{ConnectionBinding, FrameContext, FrameOutcome, RegistrationGuard};
use crate::runtime::paths::BundleRuntimePaths;

/// Connection-level binding established from a verified Hello identity.
pub(super) struct HelloBinding {
    pub(super) session_type: SessionType,
    /// Canonical `principal_id` of the connecting principal; the registry key.
    pub(super) principal_id: String,
    /// Bound bundle for session principals; `None` for relay-wide principals,
    /// whose requests must carry an explicit target bundle.
    pub(super) bound_bundle: Option<BundleRuntimePaths>,
    /// True when a store-backed credential verified the identity; false for
    /// accepted socket-trust connections. Distinguishes authenticated senders
    /// from socket-trust ones for sender-attribution responses.
    pub(super) store_backed: bool,
    /// Introspection rights for an application principal; `None` for every
    /// other principal type. Recorded on the connection context so request
    /// dispatch can gate `IdentityIntrospect`.
    pub(super) introspect_rights: Option<IdentityIntrospectRights>,
    /// Cross-relay ingress scope for a peer relay (`<id>@RELAY`) principal;
    /// `None` for every other principal type. Recorded on the connection context
    /// so a forwarded `Send`/`Raww` from this peer is gated to its scope.
    pub(super) ingress_scope: Option<String>,
}

/// Verifies a Hello, registers the stream under the resolved identity, and
/// records the resulting binding for every frame that follows.
pub(super) async fn handle_hello(
    hello: HelloFrame,
    frame: &FrameContext<'_>,
    guard: &mut RegistrationGuard,
    binding_state: &mut ConnectionBinding,
    revoke: &StreamRevokeSignal,
) -> Result<FrameOutcome, io::Error> {
    let writer = frame.writer;
    let bundle_catalog = &frame.context.bundle_catalog;
    let binding = match resolve_hello_binding(
        frame.configuration_roots,
        frame.state_root,
        bundle_catalog,
        frame.context.require_session_credentials,
        &hello,
    ) {
        Ok(binding) => binding,
        Err(error) => {
            write_stream_frame_to_writer(
                writer,
                OutgoingFrame::Response {
                    request_id: None,
                    response: &RelayResponse::Error { error },
                },
            )?;
            return Ok(FrameOutcome::Stop);
        }
    };
    // Verified `principal_id` of a store-backed connection; `None`
    // for socket-trust. Recorded on the registry entry so a
    // `change psk` rotation can find and revoke this connection.
    let connection_identity = binding.store_backed.then(|| hello.principal_id.clone());
    // Introspection scope of an application principal, recorded on
    // the registry entry so revocation fan-out can filter watching
    // hosts by scope; `None` for every other principal type.
    let connection_scope = binding
        .introspect_rights
        .as_ref()
        .and_then(|rights| rights.scope.clone());
    match register_stream(
        binding.principal_id.as_str(),
        binding.session_type,
        writer.clone(),
        connection_identity.clone(),
        revoke.clone(),
        connection_scope,
    )? {
        RegisterStreamOutcome::Registered(value) => {
            guard.set(value);
        }
        RegisterStreamOutcome::IdentityClaimConflict {
            existing_connection_id,
        } => {
            let error = identity_claim_conflict_error(&hello, existing_connection_id);
            write_stream_frame_to_writer(
                writer,
                OutgoingFrame::Response {
                    request_id: None,
                    response: &RelayResponse::Error { error },
                },
            )?;
            return Ok(FrameOutcome::Stop);
        }
    }
    write_stream_frame_to_writer(
        writer,
        OutgoingFrame::HelloAck {
            schema_version: SCHEMA_VERSION,
            principal_id: hello.principal_id.as_str(),
        },
    )?;
    if binding.session_type == SessionType::Ui
        && let Err(error) =
            emit_registration_choices_snapshots(frame.configuration_roots, bundle_catalog, &binding)
    {
        write_stream_frame_to_writer(
            writer,
            OutgoingFrame::Response {
                request_id: None,
                response: &RelayResponse::Error { error },
            },
        )?;
        return Ok(FrameOutcome::Stop);
    }
    binding_state.authenticated_identity = connection_identity;
    // The identity the connection was admitted under, credential-backed or not.
    // Cross-relay forwarding attributes the origin from this, so a relay
    // accepting socket-trust still tells a peer who a message is from.
    binding_state.admitted_identity = Some(hello.principal_id.clone());
    binding_state.introspect_rights = binding.introspect_rights;
    binding_state.ingress_scope = binding.ingress_scope;
    binding_state.bound_bundle = binding.bound_bundle;
    // A trusted-host (application principal) receives an
    // `identity.snapshot` of the active principals within its scope
    // immediately after Hello, so it can seed its view without an
    // initial introspect round-trip. Other principal types carry no
    // introspect rights and get no snapshot.
    if let Some(rights) = binding_state.introspect_rights.as_ref() {
        match handlers::build_identity_snapshot_event(
            frame.state_root,
            hello.principal_id.as_str(),
            rights,
        ) {
            Ok(event) => {
                write_stream_frame_to_writer(writer, OutgoingFrame::Event { event: &event })?
            }
            Err(error) => {
                write_stream_frame_to_writer(
                    writer,
                    OutgoingFrame::Response {
                        request_id: None,
                        response: &RelayResponse::Error { error },
                    },
                )?;
                return Ok(FrameOutcome::Stop);
            }
        }
    }
    Ok(FrameOutcome::Next)
}
