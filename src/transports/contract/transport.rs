//! The transport traits themselves: what a concrete transport must implement.
//!
//! [`Transport`] is the delivery contract, [`GenerationFence`] the three-step
//! fence protocol it builds on, and [`OutputView`] the concurrently-readable
//! handle the `look` path reads. [`TransportHealth`] and its
//! [`UnreachableSince`] latch carry the reachability axis.
//!
//! What a transport declares about the units it writes no longer appears here.
//! Under the push model a relay-injected `PartitionSink` was how a transport told
//! the guard which members shared one write, because the relay handed envelopes
//! over one at a time and could not see what became of them. The pull model has
//! no such gap: a transport declares the exact range it is about to write
//! through the [`MailboxConsumer`](super::MailboxConsumer) it already consumes
//! its mailbox with, which is the same pre-effect binding at the same point in
//! the sequence, made through the seam that was going to exist anyway.
//!
//! The shared delivery types these signatures name live in the parent
//! [`contract`](super) module, and the [`TransportImpl`](super::TransportImpl)
//! dispatch enum in [`dispatch`](super::dispatch). Nothing here imports
//! `crate::relay`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::protocol::LookSnapshotPayload;

use super::{LookMode, StartupContext, TransportError, TransportStatus};

/// The three actions a generation supervisor needs to fence a transport
/// generation, split out of [`Transport`] so the fence protocol can be driven
/// against anything that can be stopped and observed.
///
/// A generation SHALL be torn down and fenced before its replacement begins, so
/// an old generation cannot submit after its `Authorized` entries were resolved
/// against it. Without that, "resolved unknown" and "still able to act" coexist,
/// which is a target-side ordering hazard.
///
/// **Marking a generation fenced is not a fence.** A submission already past its
/// check will still produce its effect. Only *observed cessation* — through
/// [`generation_ceased`](Self::generation_ceased) — establishes that execution
/// has stopped.
///
/// None of the three has a default body. A defaulted
/// [`generation_ceased`](Self::generation_ceased) is the dangerous one: `true`
/// would acknowledge a fence no one observed, releasing a replacement while the
/// old generation can still write, and `false` would make every fence negative
/// and every target permanently unreplaceable. Neither is a safe thing to get by
/// forgetting to implement it.
pub trait GenerationFence {
    /// Step 1 — cooperative stop request. Marks the generation fenced so an
    /// executor that checks the flag stops at its next check.
    ///
    /// A signal, not a wait. It costs nothing when it works, which is why it is
    /// tried before the destructive step 3: escalating straight to termination
    /// would destroy a child that was about to stop on its own.
    fn fence_generation(&mut self);

    /// Step 3 — forced generation termination. Initiates cessation of every
    /// effect path this generation owns and returns **without blocking**.
    ///
    /// Not "kill the child": that is one implementation and does not generalise
    /// to a transport owning no child, or one reaching its target through a
    /// process it does not own. Tmux in particular SHALL NOT terminate the tmux
    /// server, which belongs to the operator rather than to the generation.
    ///
    /// Invoking it successfully does **not** acknowledge the fence. It initiates;
    /// step 4 observes. Its value is that it unblocks an executor blocked writing
    /// into the terminated path, so step 4's observation can succeed where step
    /// 2's could not.
    fn terminate_generation(&mut self);

    /// Steps 2 and 4 — the cessation observation. Whether every
    /// generation-owned executor has been observed to cease.
    ///
    /// Non-blocking, and deliberately not a join: no runtime primitive can force
    /// a thread blocked in a syscall to return, so a blocking join would
    /// reintroduce the unbounded wait the fence bound exists to close. The
    /// supervisor polls this and gives up on its own clock.
    fn generation_ceased(&self) -> bool;
}

/// Whether a transport can reach its target at all — the health axis, distinct
/// from the readiness its executor checks before a write.
///
/// Two findings were previously collapsed into a single `false` from the
/// readiness predicate. "Observed, and not ready" means the target is busy,
/// composing, or mid-turn, and waiting is correct. "Could not observe" means the
/// transport cannot reach its target, and waiting learns nothing — a member
/// queued for a departed tmux session or a permanently failed ACP worker would
/// wait forever under an unbounded `Pending`.
///
/// The `since` instant is what makes the relay's dwell threshold meaningful: the
/// bound is on *continuous* unreachability, so the transport reports when it
/// began and the relay owns how long is too long. That split keeps determination
/// in the transport and policy in the relay, with no back-edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportHealth {
    /// The transport can reach its target. Says nothing about readiness.
    Healthy,
    /// The transport cannot observe or reach its target, first seen at `since`.
    Unreachable {
        /// When unreachability was *first* observed, not when it was last
        /// checked. Restarting this on every observation would make a
        /// continuous-unreachability bound unelapsable.
        since: Instant,
    },
}

/// Latches the instant a transport first observed itself unreachable.
///
/// Exists so each transport turns a momentary observation into the contract's
/// level without repeating the latch, and so the property the dwell threshold
/// depends on — first-observed, cleared on any recovery — is defined in exactly
/// one place rather than five.
#[derive(Debug, Default)]
pub struct UnreachableSince(Mutex<Option<Instant>>);

impl UnreachableSince {
    /// Folds one observation into the reported level.
    ///
    /// A `true` (reachable) clears the latch, so an unreachability that ends
    /// before the relay's threshold elapses leaves nothing behind and resolves
    /// no members. A `false` latches the first instant and keeps returning it.
    #[must_use]
    pub fn fold(&self, reachable: bool) -> TransportHealth {
        let mut since = self.0.lock().expect("unreachable-since mutex poisoned");
        if reachable {
            *since = None;
            return TransportHealth::Healthy;
        }
        TransportHealth::Unreachable {
            since: *since.get_or_insert_with(Instant::now),
        }
    }
}

/// Delivery contract implemented by each concrete transport.
///
/// **There is no delivery method here.** The relay does not invoke a transport
/// to deliver and does not read a readiness level from one: each transport owns
/// a single serial delivery-loop executor, spawned during
/// [`startup`](Transport::startup), which consumes its target's mailbox through
/// the [`MailboxConsumer`](super::MailboxConsumer) handle it was constructed
/// with. `mailw`, `raww` and `is_ready_for_handover` are gone with the push
/// model, as the legacy synchronous `deliver`/`prepare_delivery`/`raw_write`
/// were before them.
///
/// What remains is what the relay genuinely still asks of a transport: establish
/// a runtime, tear one down, report whether the target can be reached at all,
/// publish a handle the `look` path can read, and answer the fence.
///
/// A transport MAY still keep an internal readiness predicate — the prompt-
/// readiness templates still exist and still govern Tmux and Pty — but that
/// predicate now informs only its own executor's choice of when to write what it
/// peeked, and appears on no relay-facing call.
pub trait Transport: GenerationFence {
    /// Establishes (or re-establishes, on respawn) the transport runtime for a
    /// target. On respawn the transport may publish a fresh [`OutputView`]; the
    /// worker re-calls [`give_output`] afterward to pick up the new handle.
    ///
    /// Establishing synchronously here is a choice, not an obligation. A
    /// transport whose establish is unbounded work that owns a child process —
    /// ACP spawns an agent and completes a protocol handshake — supervises it as
    /// its own task instead, and declines this call rather than offering a second
    /// route to the same child that no supervisor is watching.
    ///
    /// [`give_output`]: Transport::give_output
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError>;

    /// A monotonic marker that advances when bytes reach the target's terminal.
    ///
    /// Unlike readiness this **does** get a default, and the default is sound
    /// rather than a guess: `0` never advances, so a transport that does not
    /// track activity is simply never suppressed on this basis. That is the
    /// specified fallback, and it is why a transport with no such primitive can
    /// ignore this method entirely.
    ///
    /// Its **absence carries no meaning**. A target that is quiet may be hung,
    /// may be waiting on an operator, or may be thinking, and nothing here
    /// distinguishes them — which is why this signal can only ever withhold a
    /// write, never resolve an outcome.
    fn activity_generation(&self) -> u64 {
        0
    }

    /// Reports whether this transport can reach its target at all.
    ///
    /// The second axis beside the transport's own internal write-readiness, and
    /// a different question: readiness says *when* writing is useful, health says
    /// *whether* it is possible. A busy target and a departed one both fail a
    /// readiness check, and only the first is a reason to wait.
    ///
    /// Both are now the transport's own and neither is relay-facing. This one
    /// stays on the contract because the relay owns the *threshold* — the dwell
    /// past which a continuously unreachable target's entries resolve — even
    /// though it owns none of the observing.
    ///
    /// Also no default body, for the same shape of reason. Defaulting to
    /// [`TransportHealth::Healthy`] lets a transport that cannot observe its
    /// target claim it is fine, which is the exact failure this axis exists to
    /// close; defaulting to `Unreachable` bounces everything.
    ///
    /// Implementations SHOULD fold their momentary observation through an
    /// [`UnreachableSince`] latch rather than reporting `Instant::now()` each
    /// call, since the relay's dwell threshold measures *continuous*
    /// unreachability and a restarting clock never elapses.
    fn health(&self) -> TransportHealth;

    /// Tears down the transport runtime, releasing its resources.
    fn shutdown(&mut self);

    /// Hands the relay a concurrently-readable [`OutputView`] handle for the
    /// `look` request path, or `None` for transports with no observable output.
    ///
    /// The look request runs concurrently with the worker that owns the
    /// transport, so it cannot call [`Transport`] methods directly; the handle
    /// is the shared seam it reads instead. The worker re-fetches the handle
    /// after every [`startup`] (ACP respawn allocates a fresh replay buffer).
    ///
    /// [`startup`]: Transport::startup
    fn give_output(&self) -> Option<Arc<dyn OutputView>>;
}

/// A concurrently-readable view of a transport's output for the `look` request
/// path, published by [`Transport::give_output`].
///
/// The relay stores the handle per-target and reads it from the look request
/// thread, which runs concurrently with the worker that owns the transport. The
/// handle owns the bounded prime-wait: [`look`] reads the transport's shared
/// readiness signal, waits up to [`LookMode::prime_timeout`] for a still-
/// initializing target to populate its first snapshot, then returns the entries
/// plus freshness metadata. The relay supplies only the timeout value (its
/// look-surface policy) and remains transport-generic.
///
/// [`look`]: OutputView::look
pub trait OutputView: Send + Sync {
    /// Captures a snapshot of the target's current output.
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError>;
}
