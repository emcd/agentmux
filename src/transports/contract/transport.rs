//! The transport traits themselves: what a concrete transport must implement.
//!
//! [`Transport`] is the delivery contract, [`GenerationFence`] the three-step
//! fence protocol it builds on, [`OutputView`] the concurrently-readable handle
//! the `look` path reads, and [`PartitionSink`] the relay-injected seam a
//! transport declares its chosen partition through. [`TransportHealth`] and its
//! [`UnreachableSince`] latch carry the reachability axis.
//!
//! The shared delivery types these signatures name live in the parent
//! [`contract`](super) module, and the [`TransportImpl`](super::TransportImpl)
//! dispatch enum in [`dispatch`](super::dispatch). Nothing here imports
//! `crate::relay`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::protocol::{LookSnapshotPayload, PackingUnitId, PartitionError, SubmissionEvidence};

use super::{
    DeliveryEnvelope, LookMode, OutcomeFuture, StartupContext, TransportError, TransportStatus,
};

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
/// from handover readiness.
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

/// How a transport reports the partition it chose to the relay's guard.
///
/// The relay hands over one envelope at a time and cannot see what a transport
/// does with them: ACP coalesces a budget group into one `session/prompt`, Tmux
/// splits a batch into token-budgeted pastes, Pty writes each member on its own.
/// That partition decides which members share a fate, so the guard has to learn
/// it from the only layer that knows.
///
/// The two calls bracket the write:
///
/// 1. [`declare`](Self::declare) names the members about to share one submission
///    and returns the id that binds them, **before** the first target-side
///    effect. A transport that gets [`Err`] MUST produce no effect for that
///    proposed unit — the relay has already established, for at least one of
///    those members, that nothing was written, and writing anyway would make
///    that a false claim.
/// 2. [`record`](Self::record) states what the submission proved, once. It is
///    what every member of the unit resolves from, including members whose own
///    fan-out never completed.
///
/// **What this can and cannot enforce.** Everything after `record` returns is
/// structural relay state — no transport can make two members of one unit
/// disagree, because there is one record and they all read it. What it cannot
/// enforce is the ordering *before* `declare`: nothing here stops a transport
/// side-effecting first and declaring afterwards, which would report
/// `not_submitted` for bytes already on the wire. That stays a per-transport
/// boundary test, not a property of this trait.
///
/// Relay-injected as an `Arc<dyn PartitionSink>` for the same reason as ACP's
/// `MirrorStateFn` and Pty's `PtyMirrorStateFn`: `src/transports` and the
/// concrete transports may not import `crate::relay`, so the relay closes over
/// its own ledger and hands down an opaque handle.
///
/// Implementations must be usable from a blocking writer thread — `&self`,
/// `Send + Sync`, and synchronous. Deliberately not a channel send: routing a
/// declaration through a bounded queue would let a full queue stall the write
/// path, which is `agentmux:issues/pty/2`.
pub trait PartitionSink: Send + Sync {
    /// Declares that `member_ids` will share one submission, returning the id
    /// that binds them.
    ///
    /// All-or-nothing: an [`Err`] means no member was bound and the transport
    /// owes the unit no effect. A member already terminal, already bound, or no
    /// longer admitted vetoes the whole proposed unit — its groupmates then
    /// resolve `not_submitted`, which is true of them, because no effect
    /// occurred.
    ///
    /// `member_ids` must be non-empty and free of duplicates; either is refused.
    /// Both would leave the unit's record with a member count no sequence of
    /// terminalizations can bring to zero, so the record would outlive the
    /// process — a malformed declaration is rejected rather than half-honoured.
    fn declare(&self, member_ids: &[&str]) -> Result<PackingUnitId, PartitionError>;

    /// Records what the unit's submission proved. Write-once; the first record
    /// stands, because it is what any already-resolved member reported.
    fn record(&self, unit: PackingUnitId, evidence: SubmissionEvidence);
}

/// Delivery contract implemented by each concrete transport.
///
/// The non-blocking write methods ([`mailw`](Transport::mailw),
/// [`raww`](Transport::raww)) return an [`OutcomeFuture`]; each transport owns
/// its own internal delivery task and `spawn_blocking`. They are the relay's
/// only delivery seam — the legacy synchronous `deliver`/`prepare_delivery`/
/// `raw_write` methods have been removed.
// `async_fn_in_trait` returns a future whose `Send` is not implied by the trait.
// Every current impl's `is_ready_for_handover` future is `Send` (Pty's `!Send`
// `Terminal` never crosses its `probe.observe().await`; the await is on the
// snapshot channel only), and `run_async_delivery_worker` which awaits it is
// spawned, so a `!Send` future would fail at the spawn site. The `allow` is
// justified and documented; adding an explicit `Send` bound would require
// `return impl Future + Send` once that stabilizes.
#[allow(async_fn_in_trait)]
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

    /// Submits one relay-framed envelope for delivery WITHOUT blocking, returning
    /// an [`OutcomeFuture`] that resolves when the transport's internal delivery
    /// task drives this envelope to a terminal
    /// [`SingleDeliveryOutcome`](super::SingleDeliveryOutcome). The
    /// transport buffers the envelope on its own ordered channel, may coalesce
    /// contiguous envelopes into one target-side write, and resolves the future
    /// once that write settles.
    ///
    /// The relay's sole envelope-delivery seam; see the module-level "Write
    /// boundary" note. Default body is an additions-only stub; each transport
    /// overrides it with its internal delivery task.
    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let _ = envelope;
        unimplemented!("mailw lands with the per-transport internal delivery task")
    }

    /// Submits raw input (no envelope framing) for `raww` WITHOUT blocking,
    /// returning an [`OutcomeFuture`] that resolves when the write settles. FIFO
    /// with [`mailw`](Transport::mailw) on the transport's internal channel: a raw
    /// item flushes any buffered envelope group first, then delivers as its own
    /// write, acting as a batch barrier.
    ///
    /// The relay's sole raw-input delivery seam. Default body is an
    /// additions-only stub overridden when the internal delivery task lands.
    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let _ = (content, append_enter);
        unimplemented!("raww lands with the per-transport internal delivery task")
    }

    /// Reports whether the transport can accept a handover now.
    ///
    /// This is a level-triggered, advisory observation. A caller must still
    /// handle a fallible delivery attempt after reading it.
    ///
    /// The contract's only readiness predicate, deliberately. An earlier
    /// `is_ready` answered a weaker question — whether the transport's machinery
    /// existed — and the two were easy to confuse precisely because "ready" does
    /// not say what for. Each transport still keeps whatever lifecycle predicate
    /// it needs privately; what does not belong here is a second contract-level
    /// answer competing for the same word.
    ///
    /// It has no default body, and no surface here could supply one: a default
    /// of `true` authorizes a busy target straight into the watchdog, and a
    /// default of `false` strands it permanently, since `Pending` is unbounded. A
    /// transport answers for itself or does not participate in delivery.
    ///
    /// Async because the Pty prompt probe performs its snapshot handshake via
    /// the worker thread's `mpsc`/`oneshot` channel, which must not block a
    /// tokio worker thread (`PtyPromptProbe::observe` at `src/pty/state.rs`).
    /// The gate in `worker.rs` awaits this, so `submit_batch` holds no
    /// `&mut` borrows across a `spawn_blocking` restructuring.
    async fn is_ready_for_handover(&self) -> bool;

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
    /// handover, never resolve an outcome.
    fn activity_generation(&self) -> u64 {
        0
    }

    /// Reports whether this transport can reach its target at all.
    ///
    /// The second axis beside [`is_ready_for_handover`](Self::is_ready_for_handover),
    /// and a different question: readiness says *when* a handover is useful,
    /// health says *whether* one is possible. A busy target and a departed one
    /// both fail a readiness check, and only the first is a reason to wait.
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
