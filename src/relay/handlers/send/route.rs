use crate::configuration::{BundleConfiguration, ConfigurationRoots};
use crate::relay::authorization::AuthorizationContext;
use crate::relay::handlers::sender::SenderIdentity;
use crate::relay::routing::{
    ResolvedRoute, ResolvedTarget as RouteTarget, resolve_send_route,
};
use crate::relay::{
    BundleCatalog, RelayError, SenderReturnRoute, relay_error,
};

use super::groups::{DeliveryGroup, assemble_delivery_groups};

/// Resolves the sender's own delivery context so a non-delivered terminal
/// outcome can be routed back to it as a terminal-outcome receipt. Returns the
/// sender's *real* home-bundle member (its true transport, not the synthetic
/// `SenderIdentity::to_bundle_member` Tmux stub) plus the sender bundle's runtime
/// directory. `None` when the sender has no home bundle (`GLOBAL`/`RELAY`) or is
/// not a configured bundle member (a registered UI session): those senders learn
/// terminal outcomes through the `delivery_outcome` stream frame, not a coder
/// receipt.
pub(super) fn resolve_sender_return_route(
    home_bundle: Option<&BundleConfiguration>,
    sender_session: &str,
    home_namespace: &str,
    bundle_catalog: &BundleCatalog,
) -> Option<SenderReturnRoute> {
    let member = home_bundle?
        .members
        .iter()
        .find(|member| member.id == sender_session)?
        .clone();
    let runtime_directory = bundle_catalog
        .lookup(home_namespace)?
        .runtime_directory
        .clone();
    Some(SenderReturnRoute {
        member,
        runtime_directory,
    })
}

/// Builds the config-free [`ResolvedRoute`] for a `Send`: `resolve_send_route`
/// over the supplied targets, or — for a broadcast — every home-bundle member
/// except the sender. Broadcast requires a bundle-bound sender.
pub(super) fn resolve_send_route_or_broadcast(
    broadcast: bool,
    home_namespace: &str,
    sender: &SenderIdentity,
    home_bundle: Option<&BundleConfiguration>,
    targets: &[String],
) -> Result<ResolvedRoute, RelayError> {
    if !broadcast {
        return resolve_send_route(
            home_namespace,
            sender.session_id.as_str(),
            targets,
        );
    }
    let Some(home_bundle) = home_bundle else {
        return Err(relay_error(
            "validation_invalid_arguments",
            "broadcast requires a bundle-bound sender",
            None,
        ));
    };
    let route_targets = home_bundle
        .members
        .iter()
        .filter(|member| member.id != sender.session_id)
        .map(|member| RouteTarget {
            namespace: home_namespace.to_string(),
            session_id: Some(member.id.clone()),
            relay_id: None,
        })
        .collect();
    Ok(ResolvedRoute {
        dispatch_namespace: home_namespace.to_string(),
        requester_session: sender.session_id.clone(),
        targets: route_targets,
    })
}

/// Assembles the per-namespace delivery groups for a `Send`'s **local** targets.
/// Cross-relay (bang-path) targets are excluded — their foreign bundle is not in
/// this relay's catalog, so they are validated and delivered by the peer-forward
/// path in `execute_send`, never the local catalog. Local target existence is
/// folded into `assemble_delivery_groups`; a broadcast route's targets are
/// home-bundle members and resolve through the catalog like any other
/// bundle-bound target. Runs as the spine's `prepare` stage, before
/// authorization.
pub(super) fn prepare_send(
    route: &ResolvedRoute,
    authorization: &AuthorizationContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let local_targets = route
        .targets
        .iter()
        .filter(|target| !target.is_cross_relay())
        .cloned()
        .collect::<Vec<_>>();
    assemble_delivery_groups(
        authorization,
        configuration_roots,
        bundle_catalog,
        &local_targets,
    )
}
