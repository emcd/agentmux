//! Public relay dispatch handlers: typed operations over verified principal
//! context, routing through the same authorize and admit spine as the
//! socket transport.
//!
//! Each handler validates its context through [`check_context`], fuses
//! bare targets against the explicit operation bundle and the principal
//! home, and delegates to the internal routed handlers. Relay ingress
//! stays on the socket transport until the adapter cutover; the
//! registration-minted branch lands with its own generation-bound
//! validation.

use uuid::Uuid;

use super::identity::{PrincipalType, classify_principal_id};
use super::peer_connection::PeerConnectionManager;
use super::runtime::{RelayRuntime, VerifiedPrincipal, check_context};

/// Typed send operation for public dispatch: targets, message, broadcast
/// flag, and the explicit operation-bundle parameter driving bare-target
/// fusion and broadcast scope. The sender is the verified context itself,
/// never a wire field; relay ingress stays on the socket path.
#[derive(Clone, Debug)]
#[cfg(feature = "experimental-public-send")]
pub struct SendOperation {
    /// Raw targets as named by the caller; bare names fuse against the
    /// operation bundle, then the principal home.
    pub targets: Vec<String>,
    /// The message payload.
    pub message: String,
    /// Fan-out to the operation-bundle scope instead of named targets.
    /// Requires the operation-bundle parameter for every principal kind.
    pub broadcast: bool,
    /// Explicit default namespace for bare targets and broadcast scope.
    pub operation_bundle: Option<String>,
}

/// Dispatches a send through the public handler contract for the
/// credential-backed branch: validates the verified context, fuses bare
/// targets, and routes through the same authorize and admit spine as the
/// socket path. Relay-ingress principals and registration-minted contexts
/// are refused here; ingress stays on the socket transport until the
/// adapter cutover, and the registration branch lands with its own
/// generation-bound validation.
///
/// # Errors
///
/// Returns the [`check_context`] errors for foreign, stale, expired, or
/// post-shutdown contexts, the fusion rejections for unresolvable or
/// parameter-less broadcast targets, and the routing, authorization, and
/// admission errors of the send spine.
/// Public send dispatch: verifies the caller's context, fuses bare targets
/// against the explicit operation bundle, and routes through the shared
/// authorize-and-admit spine. Available only under the
/// `experimental-public-send` cutover feature; without it this symbol does
/// not exist and no embedding host can reach the path.
#[cfg(feature = "experimental-public-send")]
pub fn dispatch_send(
    runtime: &RelayRuntime,
    context: &VerifiedPrincipal,
    bundle_catalog: &super::BundleCatalog,
    peer_connection_manager: &PeerConnectionManager,
    operation: SendOperation,
) -> super::RelayResponse {
    if let Err(error) = check_context(runtime, context) {
        return super::RelayResponse::Error { error };
    }
    if classify_principal_id(context.principal_id())
        == Some(PrincipalType::Relay)
    {
        return super::RelayResponse::Error {
            error: crate::relay::relay_error(
                "validation_invalid_arguments",
                "relay-ingress dispatch stays on the socket transport",
                None,
            ),
        };
    }
    let home_namespace = context.home_namespace().to_string();
    let fused = match super::routing::qualify_targets_for_dispatch(
        &operation.targets,
        operation.broadcast,
        operation.operation_bundle.as_deref(),
        home_namespace.as_str(),
    ) {
        Ok(targets) => targets,
        Err(error) => return super::RelayResponse::Error { error },
    };
    // Broadcast with a parameter equal to the requester home flows through
    // the existing expansion verbatim; a foreign scope expands here to
    // explicit member targets so authorization still runs in the home
    // namespace under the cross-bundle tier rules.
    let (targets, broadcast) = match operation.operation_bundle.as_deref() {
        Some(scope)
            if operation.broadcast && scope != home_namespace.as_str() =>
        {
            match super::routing::expand_broadcast_scope(
                runtime.configuration_roots(),
                scope,
                context.principal_id(),
                home_namespace.as_str(),
            ) {
                Ok(targets) => (targets, false),
                Err(error) => return super::RelayResponse::Error { error },
            }
        },
        _ => (fused, operation.broadcast),
    };
    let request = super::RelayRequest::Send {
        request_id: Some(Uuid::new_v4().to_string()),
        requester_session: context.principal_id().to_string(),
        message: operation.message,
        targets,
        broadcast,
        on_behalf_of: None,
    };
    let principal = super::context::RequestPrincipal::from_verified(context);
    match super::handlers::handle_send_routed(
        runtime.delivery_context(),
        home_namespace.as_str(),
        request,
        runtime.configuration_roots(),
        bundle_catalog,
        Some(&principal),
        Some(peer_connection_manager),
        None,
        runtime.state_root(),
    ) {
        Ok(value) => value,
        Err(error) => super::RelayResponse::Error { error },
    }
}
