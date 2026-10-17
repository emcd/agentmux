use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use crate::configuration::{BundleConfiguration, ConfigurationRoots};
use crate::relay::authorization::{
    AuthorizationContext, choose_authorized_ui_sessions, has_ui_session,
    load_authorization_context,
};
use crate::relay::catalog::BundleCatalog;
use crate::relay::lifecycle::load_hosted_bundle;
use crate::relay::routing::ResolvedTarget as RouteTarget;
use crate::relay::{
    GLOBAL_NAMESPACE, RelayError, SCHEMA_VERSION, canonical_session_id,
    relay_error,
};

/// One namespace-scoped delivery group: a target bundle's configuration plus
/// the runtime context and choice deciders used to dispatch its targets.
pub(super) struct DeliveryGroup {
    pub(super) bundle: BundleConfiguration,
    pub(super) runtime_directory: PathBuf,
    pub(super) choice_decider_sessions: Vec<String>,
    pub(super) targets: Vec<ResolvedTarget>,
}

/// One validated target within a delivery group. Relay-wide (`@GLOBAL`) targets
/// land in the synthetic `GLOBAL` group; the delivery layer re-derives their
/// stream-vs-coder binding from the unified registry by canonical principal id.
pub(super) struct ResolvedTarget {
    pub(super) session_id: String,
}

/// Reason a `@<bundle>` target could not be resolved to a delivery group.
pub(super) enum BundleGroupError {
    /// The named bundle is not configured on this relay; the caller folds this
    /// into the request's accumulated `validation_unknown_target` set.
    UnknownBundle,
    /// Loading the bundle's configuration or authorization failed.
    Relay(RelayError),
}

/// Assembles per-namespace delivery groups from an already-classified route (the
/// config-free `MultiTarget` resolution stage in `routing.rs`). Validates target
/// existence — bundle membership or a registered UI session — and folds unknown
/// targets into a single `validation_unknown_target`. Every bundle-bound target
/// (the sender's home included) resolves its delivery context from the catalog;
/// relay-wide (`@GLOBAL`) targets are delivered via the registry and land in a
/// synthetic `GLOBAL` group.
pub(super) fn assemble_delivery_groups(
    home_authorization: &AuthorizationContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    route_targets: &[RouteTarget],
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let mut group_order: Vec<String> = Vec::new();
    let mut groups_by_bundle: HashMap<String, DeliveryGroup> = HashMap::new();
    let mut unknown_targets: Vec<String> = Vec::new();

    for target in route_targets {
        let session_id = target.session_id.as_deref().unwrap_or_default();
        if target.is_relay_wide() {
            // Relay-wide `@GLOBAL` target: existence is a registered UI session,
            // resolved from the sender's (operator) authorization context.
            if has_ui_session(home_authorization, session_id) {
                let group_key = ensure_relay_wide_group(
                    &mut group_order,
                    &mut groups_by_bundle,
                );
                push_target(
                    &mut groups_by_bundle,
                    group_key.as_str(),
                    ResolvedTarget {
                        session_id: session_id.to_string(),
                    },
                );
            } else {
                unknown_targets.push(session_id.to_string());
            }
            continue;
        }
        let namespace = target.namespace.as_str();
        match ensure_bundle_group(
            namespace,
            configuration_roots,
            bundle_catalog,
            &mut group_order,
            &mut groups_by_bundle,
        ) {
            Ok(()) => {
                let is_member =
                    groups_by_bundle.get(namespace).is_some_and(|group| {
                        group
                            .bundle
                            .members
                            .iter()
                            .any(|member| member.id == session_id)
                    });
                if is_member {
                    push_target(
                        &mut groups_by_bundle,
                        namespace,
                        ResolvedTarget {
                            session_id: session_id.to_string(),
                        },
                    );
                } else {
                    unknown_targets
                        .push(canonical_session_id(session_id, namespace));
                }
            },
            Err(BundleGroupError::UnknownBundle) => {
                unknown_targets
                    .push(canonical_session_id(session_id, namespace));
            },
            Err(BundleGroupError::Relay(error)) => return Err(error),
        }
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_target",
            "one or more targets are not canonical configured target identifiers",
            Some(json!({ "unknown_targets": unknown_targets })),
        ));
    }

    // Preserve target-discovery order and drop any seeded group that received no
    // target (e.g. the home group when every target was a peer or relay-wide).
    Ok(group_order
        .into_iter()
        .filter_map(|namespace| groups_by_bundle.remove(&namespace))
        .filter(|group| !group.targets.is_empty())
        .collect())
}

/// Returns the delivery-group key for a relay-wide (`@GLOBAL`) target, seeding
/// the group when absent. Every sender's `@GLOBAL` targets land in the same
/// synthetic `GLOBAL` group whose bundle/runtime are inert — UI delivery routes
/// by the target's principal id through the registry.
pub(super) fn ensure_relay_wide_group(
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> String {
    let key = GLOBAL_NAMESPACE.to_string();
    if !groups_by_bundle.contains_key(key.as_str()) {
        group_order.push(key.clone());
        groups_by_bundle.insert(
            key.clone(),
            DeliveryGroup {
                bundle: BundleConfiguration {
                    schema_version: SCHEMA_VERSION.to_string(),
                    bundle_name: GLOBAL_NAMESPACE.to_string(),
                    autostart: false,
                    groups: Vec::new(),
                    members: Vec::new(),
                },
                runtime_directory: PathBuf::new(),
                choice_decider_sessions: Vec::new(),
                targets: Vec::new(),
            },
        );
    }
    key
}

/// Appends a resolved target to its bundle group. The group is guaranteed to
/// exist by the time this is called.
pub(super) fn push_target(
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
    namespace: &str,
    target: ResolvedTarget,
) {
    if let Some(group) = groups_by_bundle.get_mut(namespace) {
        group.targets.push(target);
    }
}

/// Ensures a delivery group exists for `namespace`, loading the bundle's
/// configuration and authorization from the catalog when first seen. The home
/// group (when the sender is bundle-bound) and already-seen peers are seeded, so
/// they short-circuit; an unconfigured bundle is reported so the caller can fold
/// it into `validation_unknown_target`.
pub(super) fn ensure_bundle_group(
    namespace: &str,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> Result<(), BundleGroupError> {
    if groups_by_bundle.contains_key(namespace) {
        return Ok(());
    }
    let Some(paths) = bundle_catalog.lookup(namespace) else {
        return Err(BundleGroupError::UnknownBundle);
    };
    // Held-bundle guard (issues/runtime/9): a bundle whose HostingIntent is Hold
    // stays in the catalog (so lookup succeeds) but the relay is deliberately
    // not running it. Delivering to it would spuriously spawn a worker (Pty
    // included) for a bundle the operator held. Check once at group construction,
    // before any build_worker_transport, so every future transport inherits it.
    if bundle_catalog.is_held(namespace) {
        return Err(BundleGroupError::Relay(relay_error(
            "runtime_bundle_held",
            "target bundle is held and not running",
            Some(json!({ "bundle_name": namespace })),
        )));
    }
    // Delivery loads its own copy of the bundle, so it needs the same
    // authoritative stamp bring-up applies: a Pty member started lazily by this
    // delivery is spawned from exactly these members. Loading through
    // `load_hosted_bundle` is what ties the two together — the catalog's paths
    // are required to get a bundle at all.
    let bundle = load_hosted_bundle(configuration_roots, &paths)
        .map_err(BundleGroupError::Relay)?;
    let authorization =
        load_authorization_context(configuration_roots, Some(&bundle))
            .map_err(BundleGroupError::Relay)?;
    let choice_decider_sessions =
        choose_authorized_ui_sessions(&authorization, &bundle);
    group_order.push(namespace.to_string());
    groups_by_bundle.insert(
        namespace.to_string(),
        DeliveryGroup {
            bundle,
            runtime_directory: paths.runtime_directory.clone(),
            choice_decider_sessions,
            targets: Vec::new(),
        },
    );
    Ok(())
}
