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

#[cfg(feature = "experimental-public-send")]
use super::catalog::BundleCatalog;
#[cfg(feature = "experimental-public-send")]
use super::configuration::PeerConfiguration;
use super::identity::{PrincipalType, classify_principal_id};
use super::peer_connection::PeerConnectionManager;
use super::runtime::{RelayRuntime, VerifiedPrincipal, check_context};
#[cfg(feature = "experimental-public-send")]
use super::runtime_domains::{domain_live_pair, register_domain_pair};

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

/// The domain-bound dispatch pair for public send: the shared catalog and
/// peer manager a runtime's dispatch resolves through, stapled to the
/// owning delivery state.
///
/// Fields are private and there are exactly two constructors, both of
/// which source the pair instead of accepting one: [`DispatchWiring::live`]
/// reads the domain registry (populated by the host prepare path), and
/// [`DispatchWiring::mirrored`] builds from the runtime's own roots and
/// installs on first use. No constructor takes a caller-supplied pair, so
/// a foreign catalog can never be wrapped: the only question dispatch
/// asks is whether this wiring's delivery is the dispatching runtime's
/// own, decided by [`Arc::ptr_eq`] in [`dispatch_send`].
///
/// Read-only views ([`DispatchWiring::catalog`], [`DispatchWiring::peers`])
/// expose the bound pair; they confer no authority to rebind it. Available
/// only under the `experimental-public-send` cutover feature, like the
/// dispatch entry that consumes it.
#[derive(Clone, Debug)]
#[cfg(feature = "experimental-public-send")]
pub struct DispatchWiring {
    delivery: std::sync::Arc<super::DeliveryContext>,
    catalog: BundleCatalog,
    peers: std::sync::Arc<PeerConnectionManager>,
}

#[cfg(feature = "experimental-public-send")]
impl DispatchWiring {
    /// Reads the domain's installed live pair for `runtime`. Fails closed
    /// with `validation_wiring_unregistered` when nothing was ever
    /// installed, and with `validation_wiring_initializing` while a
    /// tentative install is still unpublished: initialization ownership
    /// never grants operative send authority, so readers retry after the
    /// winner commits or aborts. Serve construction alone never installs,
    /// so a foreign pair handed to a second serve constructor can never
    /// leak in here.
    pub fn live(runtime: &RelayRuntime) -> Result<Self, super::RelayError> {
        if super::runtime_domains::domain_pair_pending(runtime.state_root()) {
            return Err(super::relay_error(
                "validation_wiring_initializing",
                "domain pair initialization in progress; retry after it commits or aborts",
                None,
            ));
        }
        let (delivery, catalog, peers) =
            domain_live_pair(runtime.state_root()).ok_or_else(|| {
                super::relay_error(
                    "validation_wiring_unregistered",
                    "no live dispatch pair installed for this runtime",
                    None,
                )
            })?;
        Ok(Self {
            delivery,
            catalog,
            peers,
        })
    }

    /// Builds the dispatch pair from the runtime's own configuration roots
    /// and installs it as the domain's live pair on first use; a later call
    /// on the same domain adopts the registered pair and discards its fresh
    /// build, so two mirrored constructions can never produce divergent
    /// wirings for one domain. The install is published immediately: the
    /// mirrored constructor's install is its commitment. A tentative host
    /// install in progress fails fast with `validation_wiring_initializing`
    /// instead of adopting the unpublished pair. Membership comes from the
    /// configured bundle groups (session-less bundles are not members); a
    /// bundle whose membership disables autostart — or a host started with
    /// autostart disabled — enters the catalog as held rather than running.
    pub fn mirrored(
        runtime: &RelayRuntime,
        peers: &[PeerConfiguration],
        autostart: bool,
    ) -> Result<Self, super::RelayError> {
        // Adopt-first: an installed domain pair is returned as-is, so a
        // second construction with equivalent-but-distinct objects can
        // never conflict with or fork the registered wiring. A tentative
        // (unpublished) pair is never adopted: initialization ownership
        // grants no send authority, so the caller retries after the
        // winner commits or aborts. Only an empty slot proceeds to build
        // and install below.
        if super::runtime_domains::domain_pair_pending(runtime.state_root()) {
            return Err(super::relay_error(
                "validation_wiring_initializing",
                "domain pair initialization in progress; retry after it commits or aborts",
                None,
            ));
        }
        if let Some((delivery, catalog, peers)) =
            domain_live_pair(runtime.state_root())
        {
            return Ok(Self {
                delivery,
                catalog,
                peers,
            });
        }
        use super::errors::map_config;
        use crate::configuration::load_bundle_group_memberships;
        use crate::runtime::paths::BundleRuntimePaths;

        let memberships =
            load_bundle_group_memberships(runtime.configuration_roots())
                .map_err(map_config)?;
        let mut hosted = Vec::with_capacity(memberships.len());
        for membership in &memberships {
            let paths = BundleRuntimePaths::resolve(
                runtime.state_root(),
                membership.bundle_name.as_str(),
            )
            .map_err(|source| {
                super::relay_error(
                    "validation_invalid_arguments",
                    "configured bundle name is not resolvable",
                    Some(serde_json::json!({
                        "bundle_name": membership.bundle_name,
                        "cause": format!("{source:?}"),
                    })),
                )
            })?;
            hosted.push((
                paths,
                if autostart && membership.autostart {
                    super::catalog::HostingIntent::Run
                } else {
                    super::catalog::HostingIntent::Hold
                },
            ));
        }
        let catalog = BundleCatalog::from_entries(hosted);
        let peers = PeerConnectionManager::from_configuration(
            runtime.state_root(),
            peers,
        );
        let peers = std::sync::Arc::new(peers);
        // A racing twin may install between the adopt check above and
        // this registration, or a host prepare may hold the slot
        // tentative: on any registration failure, fall back to adopting
        // whatever is live only if it verifiably belongs to this same
        // live domain; otherwise the original error stands. A tentative
        // host install yields no live pair, so its initializing error
        // propagates for the caller to retry after commit or abort. This
        // keeps single issuance (first install wins, later calls adopt)
        // without swallowing not-owned or stale-domain failures.
        if let Err(error) = register_domain_pair(
            runtime.delivery_context(),
            runtime.state_root(),
            catalog,
            std::sync::Arc::clone(&peers),
            runtime.configuration_roots().layers(),
        ) {
            // Adopt-on-conflict with a brand check: a racing twin's
            // install is adopted only if it belongs to this same live
            // domain; anything else propagates the original error.
            let adopted = domain_live_pair(runtime.state_root()).and_then(
                |(delivery, catalog, peers)| {
                    std::sync::Arc::ptr_eq(
                        &delivery,
                        runtime.delivery_context(),
                    )
                    .then_some((delivery, catalog, peers))
                },
            );
            if let Some((delivery, catalog, peers)) = adopted {
                return Ok(Self {
                    delivery,
                    catalog,
                    peers,
                });
            }
            return Err(error);
        }
        // Read back the registered identity so the wiring carries exactly
        // what installation published, verified against this runtime
        // below. A domain change racing installation fails closed here
        // instead of handing back another generation's pair.
        let (delivery, catalog, peers) =
            domain_live_pair(runtime.state_root()).ok_or_else(|| {
                super::relay_error(
                    "internal_unexpected_failure",
                    "dispatch pair registration did not persist",
                    None,
                )
            })?;
        if !std::sync::Arc::ptr_eq(&delivery, runtime.delivery_context()) {
            return Err(super::relay_error(
                "internal_unexpected_failure",
                "dispatch pair registration did not persist",
                None,
            ));
        }
        Ok(Self {
            delivery,
            catalog,
            peers,
        })
    }

    /// The bound shared catalog (read-only view of the registered pair).
    #[must_use]
    pub fn catalog(&self) -> &BundleCatalog {
        &self.catalog
    }

    /// The bound shared peer manager (read-only view of the registered pair).
    #[must_use]
    pub fn peers(&self) -> &std::sync::Arc<PeerConnectionManager> {
        &self.peers
    }
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
    wiring: &DispatchWiring,
    operation: SendOperation,
) -> super::RelayResponse {
    // Brand check first: the wiring's delivery must be this runtime's own.
    // A wiring built for another domain — or a stale one from before a
    // domain regeneration — is refused before any context check, routing,
    // or admission work. There is deliberately no fallback pair.
    if !std::sync::Arc::ptr_eq(runtime.delivery_context(), &wiring.delivery) {
        return super::RelayResponse::Error {
            error: crate::relay::relay_error(
                "validation_wiring_runtime_mismatch",
                "dispatch wiring was not built for this runtime",
                None,
            ),
        };
    }
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
        &wiring.catalog,
        Some(&principal),
        Some(wiring.peers.as_ref()),
        None,
        runtime.state_root(),
    ) {
        Ok(value) => value,
        Err(error) => super::RelayResponse::Error { error },
    }
}

#[cfg(all(test, feature = "experimental-public-send"))]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;

    use super::DispatchWiring;
    use crate::relay::DeliveryConfiguration;
    use crate::relay::RelayRuntime;
    use crate::relay::catalog::HostingIntent;
    use crate::runtime::paths::BundleRuntimePaths;

    fn minimal_runtime() -> (TempDir, RelayRuntime) {
        let temporary = TempDir::new().expect("temporary directory");
        let configuration_root = temporary.path().join("conf");
        let state_root = temporary.path().join("state");
        std::fs::create_dir_all(&configuration_root)
            .expect("configuration directory");
        let runtime = RelayRuntime::init(
            configuration_root,
            state_root,
            DeliveryConfiguration::default(),
        )
        .expect("runtime claims the domain");
        (temporary, runtime)
    }

    /// The configuration binding is checked before occupancy: installing
    /// foreign layers on a bound delivery with an EMPTY slot is refused
    /// with a mismatch error rather than accepted as a first declaration.
    /// This pins the check-before-occupancy order at the choke point the
    /// registry funnels every install through.
    #[test]
    fn tentative_install_with_foreign_layers_is_refused_before_occupancy() {
        use std::sync::Arc as StdArc;

        use crate::relay::catalog::BundleCatalog;
        use crate::relay::peer_connection::PeerConnectionManager;

        let (_temporary, runtime) = minimal_runtime();
        let foreign = TempDir::new().expect("temporary directory");
        let foreign_layer = foreign.path().join("conf");
        let error = runtime
            .delivery_context()
            .install_tentative_live_pair(
                BundleCatalog::default(),
                StdArc::new(PeerConnectionManager::from_configuration(
                    runtime.state_root(),
                    &[],
                )),
                std::slice::from_ref(&foreign_layer),
            )
            .expect_err("foreign layers must not install on a bound owner");
        assert_eq!(
            error.code, "validation_wiring_config_mismatch",
            "the refusal names the configuration binding: {error:?}"
        );
        assert!(
            super::domain_live_pair(runtime.state_root()).is_none(),
            "a refused install publishes nothing"
        );
    }

    /// Concurrent mirrored calls share one catalog identity: every
    /// returned wiring's catalog is the same shared map (not an equal
    /// copy), every peer manager is the same `Arc`, and an in-place
    /// mutation through one wiring is visible through all the others.
    /// A barrier aligns thread starts so the calls overlap in practice;
    /// this is concurrent identity/convergence coverage rather than a
    /// deterministic install-race proof (threads may still serialize
    /// before the empty-slot check).
    #[test]
    fn concurrent_mirrored_wirings_share_catalog_identity() {
        let (_temporary, runtime) = minimal_runtime();
        let runtime = Arc::new(runtime);
        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                DispatchWiring::mirrored(&runtime, &[], true)
            }));
        }
        let mut wirings = Vec::new();
        for handle in handles {
            wirings.push(
                handle
                    .join()
                    .expect("mirrored thread joins")
                    .expect("concurrent mirrored install converges"),
            );
        }
        for pair in wirings.windows(2) {
            assert!(
                pair[0].catalog().same_identity_as(pair[1].catalog()),
                "concurrent wirings share one catalog map"
            );
            assert!(
                Arc::ptr_eq(pair[0].peers(), pair[1].peers()),
                "concurrent wirings share one peer manager"
            );
        }
        let probe = BundleRuntimePaths::resolve(runtime.state_root(), "probe")
            .expect("probe bundle paths");
        wirings[0].catalog().insert(probe, HostingIntent::Run);
        for wiring in &wirings[1..] {
            assert!(
                wiring.catalog().loaded_bundle_names().contains("probe"),
                "in-place mutation is visible through every wiring"
            );
        }
    }
}
