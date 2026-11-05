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

use super::admission::{AdmissionLedger, LedgerState, lock_ledger};
use super::async_worker::AsyncDeliveryRegistry;
use crate::relay::stream::StreamRegistry;

/// Per-handle controlled-shutdown gate.
///
/// Set when its handle begins controlled shutdown. Dispatch admission refuses
/// new work once set; worker loops keep their process-global kill-switch
/// reads. Distinct from the process signal so one handle's shutdown never
/// reaches another handle's dispatch path.
#[derive(Default)]
pub(crate) struct InstanceShutdownGate {
    signalled: AtomicBool,
}

impl InstanceShutdownGate {
    /// Signals controlled shutdown for the owning handle. Idempotent.
    pub(crate) fn signal_shutdown(&self) {
        self.signalled.store(true, Ordering::SeqCst);
    }

    /// Reports whether controlled shutdown was signalled for the handle.
    #[must_use]
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.signalled.load(Ordering::SeqCst)
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
