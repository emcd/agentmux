//! Deterministic test hooks for ingress authority/admission ordering and
//! confined-write commit boundaries.
//!
//! The authority hook pauses a peer-ingress request after preparation and
//! before its final authority resolution, so a test can commit a scope update
//! mid-flight and assert which grant governs the decision. It exists because
//! no black-box signal can pause the relay between those two steps: every
//! phase around them is fast local work.
//!
//! The hook is peer-filtered and inert unless a test arms it: unarmed calls
//! cost one mutex probe on the ingress path only, and armed gates match a
//! single peer id, so parallel tests cannot trip each other's gates.
//!
//! The commit hook runs a test-installed callback inside a confined commit
//! between staging and rename publication, so ancestor-exchange tests can
//! deterministically swap an ancestor for a symlink mid-commit. It is inert
//! unless armed: unarmed commits cost one mutex probe.

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
    pub fn arm(
        peer_principal_id: &str,
    ) -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
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

/// A rendezvous a test installs to park a session send at the final
/// admission gate — after routing and authorization, before the protected
/// revalidation span — so credential, scope, or shutdown changes commit
/// mid-flight deterministically.
pub struct SendAdmitGate {
    requester_session: String,
    reached_tx: mpsc::Sender<()>,
    proceed_rx: mpsc::Receiver<()>,
}

impl SendAdmitGate {
    /// Creates an armed gate for `requester_session`, returning the gate
    /// alongside the test-side rendezvous channels.
    pub fn arm(
        requester_session: &str,
    ) -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached_tx, reached_rx) = mpsc::channel::<()>();
        let (proceed_tx, proceed_rx) = mpsc::channel::<()>();
        (
            Self {
                requester_session: requester_session.to_string(),
                reached_tx,
                proceed_rx,
            },
            reached_rx,
            proceed_tx,
        )
    }
}

static SEND_ADMIT_GATE: Mutex<Option<SendAdmitGate>> = Mutex::new(None);

/// Installs `gate` as the active send-admit gate, replacing any previous
/// one. Test-only; call [`disarm_send_admit_gate`] when done.
pub fn arm_send_admit_gate(gate: SendAdmitGate) {
    *SEND_ADMIT_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
}

/// Removes any installed send-admit gate.
pub fn disarm_send_admit_gate() {
    *SEND_ADMIT_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Pause point for session send final admission, called after the route is
/// authorized and before the protected validity span. When a gate armed for
/// `requester_session` is installed, signals arrival and blocks until the
/// test releases it. Unarmed (or non-matching) calls return immediately.
pub(crate) fn test_send_admit_gate(requester_session: &str) {
    fire_send_admit_gate(requester_session, &SEND_ADMIT_GATE);
}

/// Pause point inside the protected span, after credential revalidation
/// and before grant re-resolution and admission. Only lock-free
/// invalidations can commit here without deadlocking the parked span;
/// store and shutdown invalidations serialize on the held locks by
/// construction.
pub(crate) fn test_send_admit_commit_gate(requester_session: &str) {
    fire_send_admit_gate(requester_session, &SEND_ADMIT_COMMIT_GATE);
}

/// Pause point inside the protected span, after the quota reservation is
/// recorded and before it is published to the queue and worker registry.
/// Lets a test park the exact reservation-to-publication interleave: a
/// shutdown arriving here waits out the span, and the entry it publishes
/// on release precedes the barrier — never orphaned past it.
pub(crate) fn test_send_admit_reserved_gate(requester_session: &str) {
    fire_send_admit_gate(requester_session, &SEND_ADMIT_RESERVED_GATE);
}

static SEND_ADMIT_RESERVED_GATE: Mutex<Option<SendAdmitGate>> =
    Mutex::new(None);

/// Installs `gate` as the active send-admit reserved gate. Disarm with
/// [`disarm_send_admit_reserved_gate`] when done.
pub fn arm_send_admit_reserved_gate(gate: SendAdmitGate) {
    *SEND_ADMIT_RESERVED_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
}

/// Removes any installed send-admit reserved gate.
pub fn disarm_send_admit_reserved_gate() {
    *SEND_ADMIT_RESERVED_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn fire_send_admit_gate(
    requester_session: &str,
    slot: &Mutex<Option<SendAdmitGate>>,
) {
    let gate = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(gate) = gate.as_ref() else {
        return;
    };
    if gate.requester_session != requester_session {
        return;
    }
    let _ = gate.reached_tx.send(());
    gate.proceed_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("test send-admit gate released");
}

static SEND_ADMIT_COMMIT_GATE: Mutex<Option<SendAdmitGate>> = Mutex::new(None);

/// Installs `gate` as the active send-admit commit gate. Disarm with
/// [`disarm_send_admit_commit_gate`] when done.
pub fn arm_send_admit_commit_gate(gate: SendAdmitGate) {
    *SEND_ADMIT_COMMIT_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
}

/// Removes any installed send-admit commit gate.
pub fn disarm_send_admit_commit_gate() {
    *SEND_ADMIT_COMMIT_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Keyed publication capture for the test contract. Fired by the send
/// spine for every queued local target, inside the final admission span,
/// next to the queued inscription. Recording is strictly opt-in: with no
/// hook armed this is one mutex probe that stores nothing, so unarmed
/// production sends — socket or public — retain zero records. An armed
/// hook captures only its own requester session; unrelated senders are
/// dropped without storage. Lifetime is the arming: tests arm before
/// dispatch, drain with [`take_published_sends`], and disarm on guard
/// drop, so captures never accumulate across tests.
pub struct SendPublishedHook {
    requester_session: String,
    captured: Mutex<Vec<String>>,
}

impl SendPublishedHook {
    /// Creates an unarmed capture for `requester_session`; install with
    /// [`arm_send_published_hook`], drain with [`take_published_sends`].
    pub fn arm(requester_session: &str) -> Self {
        Self {
            requester_session: requester_session.to_string(),
            captured: Mutex::new(Vec::new()),
        }
    }

    fn take_for(&self, requester_session: &str) -> Vec<String> {
        if self.requester_session.as_str() != requester_session {
            return Vec::new();
        }
        std::mem::take(
            &mut self
                .captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

/// Records one published local delivery under the armed hook, if any.
/// Unarmed calls store nothing.
pub(crate) fn record_send_published(
    requester_session: &str,
    message_id: &str,
) {
    let hook = PUBLISHED_SEND_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(hook) = hook.as_ref() else {
        return;
    };
    if hook.requester_session.as_str() != requester_session {
        return;
    }
    hook.captured
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(message_id.to_string());
}

static PUBLISHED_SEND_HOOK: Mutex<Option<SendPublishedHook>> =
    Mutex::new(None);

/// Installs `hook` as the active publication capture, replacing any
/// previous one. Disarm with [`disarm_send_published_hook`] when done.
pub fn arm_send_published_hook(hook: SendPublishedHook) {
    *PUBLISHED_SEND_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
}

/// Removes any installed publication capture, dropping undrained records.
pub fn disarm_send_published_hook() {
    *PUBLISHED_SEND_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Drains the recorded publications for `requester_session`, returning
/// their message ids. With no hook armed — or a hook for another sender —
/// this is empty: unarmed production sends retain nothing.
pub fn take_published_sends(requester_session: &str) -> Vec<String> {
    let hook = PUBLISHED_SEND_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(hook) = hook.as_ref() else {
        return Vec::new();
    };
    hook.take_for(requester_session)
}

/// Pause point for the shutdown wait: fires when a shutdown reaches the
/// admission-guard write, before it blocks. Lets a test prove the waiter
/// arrived while an in-flight span still holds the guard, without any
/// timing assumption — arrival is a message, not a timeout.
pub(crate) fn test_shutdown_waiting() {
    let hook = SHUTDOWN_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hook) = hook.as_ref() {
        let _ = hook.send(());
    }
}

static SHUTDOWN_WAIT_HOOK: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);

/// Installs a one-shot arrival signal for the next shutdown wait.
pub fn arm_shutdown_wait_hook(sender: mpsc::Sender<()>) {
    *SHUTDOWN_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
}

/// Removes any installed shutdown-wait hook.
pub fn disarm_shutdown_wait_hook() {
    *SHUTDOWN_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Arrival signal for a store mutation reaching its transaction-lock
/// acquisition. Lets a test prove a rotation/revocation mutator arrived
/// at the store mutex — rather than merely being spawned — before
/// asserting it cannot commit while a send span holds that mutex.
///
/// Keyed by store root and principal id, and one-shot: only a firing
/// whose key matches the armed key signals, and that firing consumes
/// the arming. Arrivals from unrelated runtimes or principals neither
/// signal nor consume, so a shared-process neighbor rotating its own
/// store cannot satisfy this waiter.
pub(crate) fn test_store_mutation_waiting(
    state_root: &std::path::Path,
    principal_id: &str,
) {
    let mut hook = STORE_MUTATION_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let matches = hook.as_ref().is_some_and(|(key, _)| {
        key.state_root.as_path() == state_root
            && key.principal_id.as_str() == principal_id
    });
    if matches {
        let (_, sender) = hook.take().expect("matched hook is present");
        let _ = sender.send(());
    }
}

struct StoreMutationWaitKey {
    state_root: std::path::PathBuf,
    principal_id: String,
}

static STORE_MUTATION_WAIT_HOOK: Mutex<
    Option<(StoreMutationWaitKey, mpsc::Sender<()>)>,
> = Mutex::new(None);

/// Arms a one-shot arrival signal for the next store mutation whose
/// store root and principal id both match. A firing with any other key
/// is dropped without signaling and without consuming the arming.
pub fn arm_store_mutation_wait_hook(
    state_root: &std::path::Path,
    principal_id: &str,
    sender: mpsc::Sender<()>,
) {
    *STORE_MUTATION_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((
        StoreMutationWaitKey {
            state_root: state_root.to_path_buf(),
            principal_id: principal_id.to_string(),
        },
        sender,
    ));
}

/// Removes any installed store-mutation-wait hook.
pub fn disarm_store_mutation_wait_hook() {
    *STORE_MUTATION_WAIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// A one-shot callback a test installs to run inside a confined commit
/// between staging and rename publication — e.g. exchanging an ancestor
/// directory for a symlink — so ancestor-exchange tests are deterministic.
/// Inert unless armed: unarmed commits cost one mutex probe.
pub struct ConfinedCommitHook {
    callback: Box<dyn Fn() + Send + 'static>,
}

impl ConfinedCommitHook {
    /// Creates a commit hook running `callback` at the staging/commit
    /// boundary.
    pub fn arm(callback: impl Fn() + Send + 'static) -> Self {
        Self {
            callback: Box::new(callback),
        }
    }
}

static CONFINED_COMMIT_HOOK: Mutex<Option<ConfinedCommitHook>> =
    Mutex::new(None);

/// Installs `hook` as the active confined-commit hook, replacing any
/// previous one. Test-only; call [`disarm_confined_commit_hook`] when done.
pub fn arm_confined_commit_hook(hook: ConfinedCommitHook) {
    *CONFINED_COMMIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
}

/// Removes any installed confined-commit hook.
pub fn disarm_confined_commit_hook() {
    *CONFINED_COMMIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Commit-boundary pause point for confined writes, called after staging and
/// before rename publication. Runs the armed hook, if any, then returns.
/// Unarmed calls return immediately after one mutex probe.
pub(crate) fn fire_confined_commit_hook() {
    let hook = CONFINED_COMMIT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hook) = hook.as_ref() {
        (hook.callback)();
    }
}

/// A one-shot callback a test installs to run inside a domain-claim
/// acquisition between opening the lock file and verifying the retained
/// parent — e.g. exchanging the identity directory for a real replacement
/// — so acquisition-boundary exchange tests are deterministic. Inert unless
/// armed: unarmed claims cost one mutex probe.
///
/// The hook carries the state root it applies to: claims over other roots
/// (including parallel tests) never fire it, so the exchange lands strictly
/// inside the armed claim's own walk-to-verify window.
pub struct ConfinedClaimHook {
    root: std::path::PathBuf,
    callback: Box<dyn Fn() + Send + 'static>,
}

impl ConfinedClaimHook {
    /// Creates a claim hook running `callback` at the open/verify boundary
    /// of claims over `root`.
    pub fn arm_for(
        root: std::path::PathBuf,
        callback: impl Fn() + Send + 'static,
    ) -> Self {
        Self {
            root,
            callback: Box::new(callback),
        }
    }
}

static CONFINED_CLAIM_HOOK: Mutex<Option<ConfinedClaimHook>> =
    Mutex::new(None);

/// Installs `hook` as the active confined-claim hook, replacing any
/// previous one. Test-only; call [`disarm_confined_claim_hook`] when done.
pub fn arm_confined_claim_hook(hook: ConfinedClaimHook) {
    *CONFINED_CLAIM_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
}

/// Removes any installed confined-claim hook.
pub fn disarm_confined_claim_hook() {
    *CONFINED_CLAIM_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Claim-boundary pause point for domain acquisition, called after opening
/// the lock file and before verifying the retained parent. Runs the armed
/// hook when it targets `state_root`, if any, then returns. Unarmed calls,
/// and calls over other roots, return after mutex probes.
pub(crate) fn fire_confined_claim_hook(state_root: &std::path::Path) {
    let hook = CONFINED_CLAIM_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hook) = hook.as_ref() {
        let applies = hook.root == state_root
            || std::fs::canonicalize(&hook.root).ok().as_deref()
                == Some(state_root);
        if applies {
            (hook.callback)();
        }
    }
}

/// Fault to inject at an identity-rollback persist, scoped by store root so
/// a test can drive one call's compensation outcome through socket dispatch
/// (which runs on the blocking pool, where thread-gated hooks cannot target
/// a single call). One-shot: taking the fault disarms it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdentityRollbackFault {
    /// Fail the compensation before its own publication: the compensated
    /// state never lands.
    PrePublicationFailure,
    /// Publish the compensation for real, then report post-rename durability
    /// uncertainty instead of success.
    PostPublicationUncertain,
}

static IDENTITY_ROLLBACK_FAULTS: std::sync::OnceLock<
    Mutex<
        std::collections::HashMap<std::path::PathBuf, IdentityRollbackFault>,
    >,
> = std::sync::OnceLock::new();

/// Locks the rollback-fault registry, initializing it on first use.
fn identity_rollback_faults() -> &'static Mutex<
    std::collections::HashMap<std::path::PathBuf, IdentityRollbackFault>,
> {
    IDENTITY_ROLLBACK_FAULTS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Arms a one-shot rollback fault for the store under `state_root`.
/// Test-only; the fault is consumed by the next rollback persist for that
/// root, or remove it with [`disarm_identity_rollback_fault`].
pub fn arm_identity_rollback_fault(
    state_root: &std::path::Path,
    fault: IdentityRollbackFault,
) {
    let key = std::fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf());
    identity_rollback_faults()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, fault);
}

/// Removes any armed rollback fault for the store under `state_root`.
pub fn disarm_identity_rollback_fault(state_root: &std::path::Path) {
    let key = std::fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf());
    identity_rollback_faults()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&key);
}

/// Takes the armed rollback fault for the store under `state_root`, if any.
/// Called at the compensation persist site; unarmed calls cost one mutex
/// probe and return `None`.
pub(crate) fn take_identity_rollback_fault(
    state_root: &std::path::Path,
) -> Option<IdentityRollbackFault> {
    let key = std::fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf());
    identity_rollback_faults()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&key)
}

/// Runs `persist` for an identity-store compensation, overridden by an armed
/// rollback fault: a pre-publication fault skips the persist and reports a
/// synthetic pre-rename failure, while a post-publication fault persists
/// for real and then reports durability uncertainty instead of success. With
/// no fault armed this is exactly `persist()`: one mutex probe.
pub(crate) fn fault_aware_rollback_persist(
    state_root: &std::path::Path,
    principal_id: &str,
    persist: impl FnOnce() -> Result<(), super::RelayError>,
) -> Result<(), super::RelayError> {
    match take_identity_rollback_fault(state_root) {
        Some(IdentityRollbackFault::PrePublicationFailure) => {
            Err(super::relay_error(
                "internal_principal_store",
                "injected fault: identity rollback persist failed before publication",
                Some(serde_json::json!({
                    "principal_id": principal_id,
                })),
            ))
        },
        Some(IdentityRollbackFault::PostPublicationUncertain) => {
            match persist() {
                Ok(()) => Err(super::relay_error(
                    "internal_store_durability_uncertain",
                    "injected fault: identity rollback durability uncertain after publication",
                    Some(serde_json::json!({
                        "principal_id": principal_id,
                    })),
                )),
                Err(real) => Err(real),
            }
        },
        None => persist(),
    }
}
