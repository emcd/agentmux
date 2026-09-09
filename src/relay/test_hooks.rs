//! Deterministic test hooks for ingress authority/admission ordering.
//!
//! The single hook here pauses a peer-ingress request after preparation and
//! before its final authority resolution, so a test can commit a scope update
//! mid-flight and assert which grant governs the decision. It exists because
//! no black-box signal can pause the relay between those two steps: every
//! phase around them is fast local work.
//!
//! The hook is peer-filtered and inert unless a test arms it: unarmed calls
//! cost one mutex probe on the ingress path only, and armed gates match a
//! single peer id, so parallel tests cannot trip each other's gates.

use std::{
    sync::{Mutex, mpsc},
    time::Duration,
};

/// A rendezvous installed by one test for one peer principal.
pub struct AuthorityGate {
    peer_principal_id: String,
    reached_tx: mpsc::Sender<()>,
    proceed_rx: mpsc::Receiver<()>,
}

impl AuthorityGate {
    /// Creates an armed gate for `peer_principal_id`, returning the gate
    /// alongside the test-side rendezvous channels.
    pub fn arm(peer_principal_id: &str) -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached_tx, reached_rx) = mpsc::channel::<()>();
        let (proceed_tx, proceed_rx) = mpsc::channel::<()>();
        (
            Self {
                peer_principal_id: peer_principal_id.to_string(),
                reached_tx,
                proceed_rx,
            },
            reached_rx,
            proceed_tx,
        )
    }
}

static AUTHORITY_GATE: Mutex<Option<AuthorityGate>> = Mutex::new(None);

/// Installs `gate` as the active authority gate, replacing any previous one.
/// Test-only; call [`disarm_authority_gate`] when done (a peer-filtered gate
/// left armed is harmless, but tidiness keeps parallel runs legible).
pub fn arm_authority_gate(gate: AuthorityGate) {
    *AUTHORITY_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
}

/// Removes any installed authority gate.
pub fn disarm_authority_gate() {
    *AUTHORITY_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Pause point for peer ingress, called after preparation and before final
/// authority resolution. When a gate armed for `peer_principal_id` is
/// installed, signals arrival and blocks until the test releases it, so a
/// scope update can commit mid-flight deterministically. Unarmed (or
/// non-matching) calls return immediately. Timeouts fail loudly rather than
/// resolving ambiguously — a stuck gate must never be mistaken for an
/// authorization decision.
pub(crate) fn test_authority_gate(peer_principal_id: &str) {
    let gate = AUTHORITY_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(gate) = gate.as_ref() else {
        return;
    };
    if gate.peer_principal_id != peer_principal_id {
        return;
    }
    let _ = gate.reached_tx.send(());
    gate.proceed_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("test authority gate released");
}
