//! Per-handle relay delivery state.
//!
//! Everything that distinguishes one relay deployment from another — the
//! resolved `[delivery]` table, the admission ledger, the stream and async
//! worker registries, the connection drain coordinator, and the controlled
//! shutdown gate — lives in [`DeliveryContext`], owned per relay handle.
//! Nothing here is process-global: two live handles hold independent
//! instances and neither reads the other's state.
//!
//! The process-global termination signal (`runtime::signals`) is deliberately
//! *not* part of this context. It remains the process-level kill switch that
//! may legitimately stop every handle at process shutdown. A handle's
//! controlled shutdown signals its own gate instead; dispatch admission
//! consults the gate, worker loops keep the global read.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::relay::configuration::DeliveryConfiguration;
use crate::relay::drain::ConnectionDrainCoordinator;

use super::super::test_hooks;
use super::admission::{AdmissionLedger, LedgerState, lock_ledger};
use super::async_worker::AsyncDeliveryRegistry;
use crate::relay::stream::StreamRegistry;

/// Per-handle controlled-shutdown gate.
///
/// Set when its handle begins controlled shutdown. Dispatch admission refuses
/// new work once set; worker loops keep their process-global kill-switch
/// reads. Distinct from the process signal so one handle's shutdown never
/// reaches another handle's dispatch path.
///
/// Admission and shutdown linearize through the admission guard: dispatch
/// holds the guard shared across its whole check-to-commit span, and
/// shutdown publishes the gate first, then takes the guard exclusively
/// to wait out in-flight admissions. The guard is outermost in the lock
/// order (guard, admin, store, ledger), with the single documented
/// peer-ingress exception holding the identity-admin serialization
/// first (admin, then guard, then store): shutdown waits holding no inner
/// locks, and admission never touches the gate without holding the guard.
#[derive(Default)]
pub(crate) struct InstanceShutdownGate {
    signalled: AtomicBool,
    admission: std::sync::RwLock<()>,
}

impl InstanceShutdownGate {
    /// Signals controlled shutdown for the owning handle. Idempotent.
    /// Publishes the gate first, then waits out in-flight admissions
    /// before returning: a span that acquired first commits visibly,
    /// and a span that acquires after sees the published gate and
    /// refuses. No inner locks are held while waiting.
    pub(crate) fn signal_shutdown(&self) {
        self.signalled.store(true, Ordering::SeqCst);
        test_hooks::test_shutdown_waiting();
        let _held = self
            .admission
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }

    /// Reports whether controlled shutdown was signalled for the handle.
    #[must_use]
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.signalled.load(Ordering::SeqCst)
    }

    /// Holds the admission guard shared across one check-to-commit span.
    /// Release only after the commit or rollback publishes.
    pub(crate) fn admission_span(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.admission
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reports whether the admission guard is currently held shared.
    /// Test-only exclusion probe: a would-be invalidator that must
    /// acquire the guard exclusively observes held exclusion while a
    /// span is parked inside the check-to-commit region.
    pub fn exclusion_held_for_testing(&self) -> bool {
        self.admission.try_write().is_err()
    }
}

/// Relay delivery state owned by one relay handle.
///
/// Constructed once per handle from the resolved `[delivery]` table; the
/// standalone host builds one at startup and every accepted connection,
/// dispatch, and worker task shares it via `Arc`. Integration tests build
/// one per test instead of publishing process-global configuration.
///
/// This type is public so the test-facing `relay::handle_request` entry
/// point can accept it. The stable embeddable embedding boundary (opaque
/// verified context, public dispatch) arrives in a later phase; until
/// then, treat this surface as provisional.
pub struct DeliveryContext {
    configuration: DeliveryConfiguration,
    ledger: AdmissionLedger,
    stream_registry: StreamRegistry,
    async_registry: AsyncDeliveryRegistry,
    drain_coordinator: Arc<ConnectionDrainCoordinator>,
    shutdown_gate: InstanceShutdownGate,
    /// Exclusive store-root domain claim held for the delivery state's
    /// lifetime: whoever holds an `Arc` of this state — live streams, serve
    /// contexts, the embedding handle — keeps the cross-process claim alive
    /// with it, so a second process (or a second runtime in this process)
    /// cannot own the same store while this domain is still effect-capable.
    /// Released automatically when the last holder drops.
    domain_lock: std::sync::Mutex<Option<std::fs::File>>,
    /// The domain's installed dispatch pair, if any. It lives and dies
    /// with this delivery state — never in a side registry — so a dropped
    /// domain takes its pair with it and a reclaimed root starts empty by
    /// construction. Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    wiring_pair: std::sync::Mutex<
        Option<crate::relay::runtime_domains::InstalledDomainPair>,
    >,
    /// The ordered, resolved configuration-source identity bound to this
    /// delivery state when it acquired domain ownership: the absolutized
    /// configuration layers. A restarted domain may bind different
    /// configuration once all prior holders release (a fresh claim binds
    /// anew), but a live owner never rebinds: installs carrying different
    /// layers are refused, so an adopted prepare cannot silently mix one
    /// runtime's authority with another configuration's hosting.
    /// Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    bound_configuration_layers:
        std::sync::Mutex<Option<Vec<std::path::PathBuf>>>,
}

impl DeliveryContext {
    /// Builds delivery state for one relay handle from its resolved
    /// `[delivery]` table.
    #[must_use]
    pub fn new(configuration: DeliveryConfiguration) -> Self {
        Self {
            configuration,
            ledger: AdmissionLedger::default(),
            stream_registry: StreamRegistry::default(),
            async_registry: AsyncDeliveryRegistry::default(),
            drain_coordinator: ConnectionDrainCoordinator::new(),
            shutdown_gate: InstanceShutdownGate::default(),
            domain_lock: std::sync::Mutex::new(None),
            #[cfg(feature = "experimental-public-send")]
            wiring_pair: std::sync::Mutex::new(None),
            #[cfg(feature = "experimental-public-send")]
            bound_configuration_layers: std::sync::Mutex::new(None),
        }
    }

    /// The handle's resolved `[delivery]` table.
    #[must_use]
    pub fn delivery_configuration(&self) -> DeliveryConfiguration {
        self.configuration
    }
}

impl std::fmt::Debug for DeliveryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliveryContext")
            .field("configuration", &self.configuration)
            .finish_non_exhaustive()
    }
}

impl DeliveryContext {
    /// Acquires the handle's admission ledger.
    pub(crate) fn lock_ledger(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, LedgerState>, crate::relay::RelayError>
    {
        lock_ledger(&self.ledger)
    }

    /// Reads back the installed dispatch pair, cloning the shared handles.
    /// `None` means no pair was ever installed on this delivery state, or
    /// the installed pair is still tentative: preparation publishes
    /// dispatchability only at its commit boundary, after every fallible
    /// step, so concurrent readers can never obtain a wiring for a pair
    /// that may yet abort. Because the slot lives inside the delivery
    /// itself, a reclaimed root always reads a fresh slot — never a
    /// previous generation's pair. Experimental-gated with the wiring
    /// surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn read_live_pair(
        &self,
    ) -> Option<(
        crate::relay::catalog::BundleCatalog,
        Arc<crate::relay::peer_connection::PeerConnectionManager>,
    )> {
        let slot = self
            .wiring_pair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.as_ref().and_then(|installed| {
            installed.published.then(|| {
                (installed.catalog.clone(), Arc::clone(&installed.peers))
            })
        })
    }

    /// Installs the domain's live dispatch pair into this delivery state,
    /// published and immediately readable, or reports the already
    /// installed one. First installation wins per delivery lifetime; a
    /// divergent reinstall over a committed pair is refused so a peer-set
    /// change forces domain regeneration instead of silently rebinding
    /// live dispatch, while a divergent install over a still-tentative
    /// pair fails with an initializing error so the loser retries after
    /// the winner commits or aborts. The embedded mirrored constructor
    /// uses this: its install is its commitment. `install_layers` must
    /// agree with the configuration identity bound to this delivery
    /// state at domain-ownership acquisition; a mismatch is refused
    /// before anything installs, so an adopted prepare cannot mix one
    /// runtime's authority with another configuration's hosting.
    /// Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn install_live_pair(
        &self,
        catalog: crate::relay::catalog::BundleCatalog,
        peers: Arc<crate::relay::peer_connection::PeerConnectionManager>,
        install_layers: &[std::path::PathBuf],
    ) -> Result<bool, crate::relay::RelayError> {
        self.install_pair(catalog, peers, true, install_layers)
    }

    /// Installs the domain's dispatch pair into this delivery state as
    /// tentative: unreadable through live/mirrored reads until
    /// [`publish_live_pair`](Self::publish_live_pair) runs. The host
    /// prepare path uses this so the registration preflight reserves
    /// initialization ownership without granting operative send
    /// authority. Same first-installation-wins and configuration-binding
    /// rules as [`install_live_pair`](Self::install_live_pair).
    /// Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn install_tentative_live_pair(
        &self,
        catalog: crate::relay::catalog::BundleCatalog,
        peers: Arc<crate::relay::peer_connection::PeerConnectionManager>,
        install_layers: &[std::path::PathBuf],
    ) -> Result<bool, crate::relay::RelayError> {
        self.install_pair(catalog, peers, false, install_layers)
    }

    /// Binds the ordered, resolved configuration-source identity to this
    /// delivery state at domain-ownership acquisition. Called once by
    /// whoever acquires the domain (runtime construction or a claiming
    /// prepare); a live owner never rebinds, so later installs carrying
    /// different layers are refused rather than silently mixing
    /// configurations. A restarted domain binds anew once all prior
    /// holders release: this is a per-generation binding, not a
    /// permanent deployment-to-configuration mapping, and it records
    /// source identity (which layers), never content freshness.
    /// Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(crate) fn bind_configuration_identity(
        &self,
        layers: &[std::path::PathBuf],
    ) {
        let mut bound = self
            .bound_configuration_layers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if bound.is_none() {
            *bound = Some(Self::canonical_configuration_layers(layers));
        }
    }

    /// The ordered, resolved configuration layers, absolutized lexically
    /// (no symlink resolution, matching the propagation contract): two
    /// spellings resolving through the same normalization pipeline to
    /// the same list name the same source identity.
    #[cfg(feature = "experimental-public-send")]
    fn canonical_configuration_layers(
        layers: &[std::path::PathBuf],
    ) -> Vec<std::path::PathBuf> {
        layers
            .iter()
            .map(|layer| {
                std::path::absolute(layer).unwrap_or_else(|_| layer.clone())
            })
            .collect()
    }

    #[cfg(feature = "experimental-public-send")]
    fn install_pair(
        &self,
        catalog: crate::relay::catalog::BundleCatalog,
        peers: Arc<crate::relay::peer_connection::PeerConnectionManager>,
        published: bool,
        install_layers: &[std::path::PathBuf],
    ) -> Result<bool, crate::relay::RelayError> {
        // The configuration check precedes the occupancy check: a
        // mismatched installer is refused for what it carries even when
        // the slot is empty, and a matching installer keeps the
        // existing conflict/initializing behavior when it is not. Every
        // domain publisher binds before publication, so an unbound owner
        // here is a programming error, never a first declaration: fail
        // loud rather than let an unbound owner accept an arbitrary
        // installation.
        let installing = Self::canonical_configuration_layers(install_layers);
        let bound = self
            .bound_configuration_layers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match bound.as_ref() {
            Some(bound) if *bound != installing => {
                return Err(crate::relay::relay_error(
                    "validation_wiring_config_mismatch",
                    "preparation configuration does not match the owning domain's bound configuration; restart the owner with the intended configuration",
                    None,
                ));
            },
            Some(_) => {},
            None => {
                return Err(crate::relay::relay_error(
                    "internal_unexpected_failure",
                    "dispatch pair install found no bound configuration identity",
                    None,
                ));
            },
        }
        drop(bound);
        let mut slot = self
            .wiring_pair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.as_ref() {
            Some(installed) => {
                if installed.catalog.same_identity_as(&catalog)
                    && Arc::ptr_eq(&installed.peers, &peers)
                {
                    Ok(false)
                } else if installed.published {
                    Err(crate::relay::relay_error(
                        "validation_wiring_conflict",
                        "domain pair already installed; regenerate the domain to change it",
                        None,
                    ))
                } else {
                    Err(crate::relay::relay_error(
                        "validation_wiring_initializing",
                        "domain pair initialization in progress; retry after it commits or aborts",
                        None,
                    ))
                }
            },
            None => {
                *slot =
                    Some(crate::relay::runtime_domains::InstalledDomainPair {
                        catalog,
                        peers,
                        published,
                    });
                Ok(true)
            },
        }
    }

    /// Publishes a tentatively installed pair, making it readable through
    /// live/mirrored reads. The host prepare path calls this once, at its
    /// commit boundary after every fallible step: dispatchability starts
    /// here and not earlier. Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn publish_live_pair(
        &self,
    ) -> Result<(), crate::relay::RelayError> {
        let mut slot = self
            .wiring_pair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.as_mut() {
            Some(installed) => {
                installed.published = true;
                Ok(())
            },
            None => Err(crate::relay::relay_error(
                "internal_unexpected_failure",
                "dispatch pair publication found no installed pair",
                None,
            )),
        }
    }

    /// Reports whether this delivery state holds a tentative (installed
    /// but unpublished) pair. Readers use this to fail closed with an
    /// initializing error rather than treating the domain as merely
    /// unregistered. Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn pair_pending(&self) -> bool {
        let slot = self
            .wiring_pair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.as_ref().is_some_and(|installed| !installed.published)
    }

    /// Removes the installed dispatch pair from this delivery state, if
    /// any. The host prepare path calls this when preparation fails after
    /// the early tentative install, so a failed preparation never leaves
    /// a retained tentative pair behind for the next preparation to
    /// conflict with. Experimental-gated with the wiring surface.
    #[cfg(feature = "experimental-public-send")]
    pub(in crate::relay) fn clear_live_pair(&self) {
        let mut slot = self
            .wiring_pair
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    /// Holds the admission guard shared across one check-to-commit span.
    /// Shutdown publishes its gate first, then waits on the matching
    /// exclusive hold, so a span that acquired first commits visibly.
    pub(crate) fn admission_span(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.shutdown_gate.admission_span()
    }

    /// Reports whether the admission guard is currently held shared.
    /// Test-only exclusion probe: a would-be invalidator that must
    /// acquire the guard exclusively observes held exclusion while a
    /// span is parked inside the check-to-commit region.
    pub fn admission_exclusion_for_testing(&self) -> bool {
        self.shutdown_gate.exclusion_held_for_testing()
    }

    /// The handle's stream registry.
    #[must_use]
    pub(crate) fn stream_registry(&self) -> &StreamRegistry {
        &self.stream_registry
    }

    /// The handle's async worker registry.
    #[must_use]
    pub(crate) fn async_registry(&self) -> &AsyncDeliveryRegistry {
        &self.async_registry
    }

    /// The handle's connection drain coordinator.
    #[must_use]
    pub(crate) fn drain_coordinator(
        &self,
    ) -> &Arc<ConnectionDrainCoordinator> {
        &self.drain_coordinator
    }

    /// The handle's controlled-shutdown gate.
    #[must_use]
    pub(crate) fn shutdown_gate(&self) -> &InstanceShutdownGate {
        &self.shutdown_gate
    }

    /// Installs the exclusive store-root domain claim held for this delivery
    /// state's lifetime. One delivery state owns one store root: a second
    /// claim is refused so adopting a new root can never silently release
    /// the earlier root's lock file while its streams and registry entry
    /// are still live. Returns false without installing anything when this
    /// state already holds a claim.
    pub(in crate::relay) fn install_domain_lock(
        &self,
        lock: std::fs::File,
    ) -> bool {
        let mut slot = self
            .domain_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            drop(lock);
            return false;
        }
        *slot = Some(lock);
        true
    }

    /// Reports whether controlled shutdown was signalled for the handle.
    #[must_use]
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutdown_gate.is_shutting_down()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use tokio::sync::Notify;

    use crate::configuration::SessionType;
    use crate::relay::DeliveryConfiguration;

    use super::super::admission::{
        AdmissionTargetKey, admit, rollback_admission,
    };
    use super::super::async_worker::{
        build_worker_key, register_worker_if_absent, worker_exists,
    };
    use super::DeliveryContext;

    /// Two live handles hold fully independent delivery state: separate
    /// ledgers, registries, configuration, and shutdown gates. Nothing one
    /// handle admits, registers, or shuts down is observable from the other.
    ///
    /// Per-instance invariant: the per-handle `DeliveryContext` must not leak
    /// across instances through any residual process-global.
    #[test]
    fn two_handles_hold_independent_delivery_state() {
        let delivery_a =
            Arc::new(DeliveryContext::new(DeliveryConfiguration {
                queued_envelopes_max: 1,
                ..DeliveryConfiguration::default()
            }));
        let delivery_b =
            Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));

        // Configuration is per-handle.
        assert_eq!(
            delivery_a.delivery_configuration().queued_envelopes_max,
            1
        );
        assert_ne!(
            delivery_b.delivery_configuration().queued_envelopes_max,
            1,
        );

        // Admission: A and B hold independent quota counters. A admits one
        // envelope per target; a second admission in A is refused, while B —
        // which has the default quota — admits freely, proving B's ledger
        // holds none of A's reservations.
        let target = AdmissionTargetKey::new(
            "party",
            Path::new("/nonexistent"),
            "bravo",
        );
        admit(
            &delivery_a,
            "message-a-1",
            target.clone(),
            SessionType::Tmux,
            1,
        )
        .expect("first admission fits A's quota");
        assert!(
            admit(
                &delivery_a,
                "message-a-2",
                target.clone(),
                SessionType::Tmux,
                1,
            )
            .is_err(),
            "A's per-target quota is exhausted by its own reservation"
        );
        admit(
            &delivery_b,
            "message-b-1",
            target.clone(),
            SessionType::Tmux,
            1,
        )
        .expect("B is unaffected by A's reservations");
        admit(
            &delivery_b,
            "message-b-2",
            target.clone(),
            SessionType::Tmux,
            1,
        )
        .expect("B's own quota is independent of A's");
        rollback_admission(&delivery_a, "message-a-1");
        rollback_admission(&delivery_b, "message-b-1");
        rollback_admission(&delivery_b, "message-b-2");

        // Async worker registry: a registration in A is absent in B.
        let key =
            build_worker_key("party", Path::new("/nonexistent"), "bravo");
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        register_worker_if_absent(
            delivery_a.async_registry(),
            key.clone(),
            sender,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            false,
        )
        .expect("register in A");
        assert!(
            worker_exists(delivery_a.async_registry(), &key)
                .expect("registry readable"),
            "the worker is registered in A"
        );
        assert!(
            !worker_exists(delivery_b.async_registry(), &key)
                .expect("registry readable"),
            "A's worker must not appear in B's registry"
        );

        // Shutdown gate: signalling A leaves B serving.
        assert!(!delivery_a.is_shutting_down());
        assert!(!delivery_b.is_shutting_down());
        delivery_a.shutdown_gate().signal_shutdown();
        assert!(
            delivery_a.is_shutting_down(),
            "A is shutting down after its own signal"
        );
        assert!(
            !delivery_b.is_shutting_down(),
            "signalling A must not shut B down"
        );
    }

    /// Stream registration is per-handle: a connection registered against one
    /// handle is unknown to the other handle's registry.
    #[test]
    fn two_handles_hold_independent_stream_registries() {
        use crate::relay::stream::{
            lookup_registry_session_type, register_stream,
        };

        let delivery_a =
            Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
        let delivery_b =
            Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));

        let (writer, _receiver) =
            tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        register_stream(
            &delivery_a,
            "alpha@party",
            SessionType::Ui,
            writer,
            None,
            Arc::new(Notify::new()),
            None,
        )
        .expect("register in A");
        assert_eq!(
            lookup_registry_session_type(&delivery_a, "alpha@party"),
            Some(SessionType::Ui),
            "the stream is registered in A"
        );
        assert_eq!(
            lookup_registry_session_type(&delivery_b, "alpha@party"),
            None,
            "A's stream must not appear in B's registry"
        );
    }
}
