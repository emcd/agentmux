//! Observing and pausing *registered* callers at the ledger lock's boundary.
//!
//! Compiled only under `cfg(test)`, and reached only from
//! [`lock_ledger`](super::ledger::lock_ledger). It adds no locking path of its
//! own, which is what keeps it inside the rule this subsystem's hub states: the
//! ledger guard is still taken exactly once, at the head of each entry point,
//! and this observes that one acquisition rather than introducing a second.
//!
//! **Why a seam exists here at all.** The serialization this subsystem rests on
//! is a claim about an interleaving: a `declare` or `ack` already inside the
//! critical section completes before a replacement can flip the target's
//! generation. From outside the lock that interleaving is unobservable. Two
//! threads released together prove nothing — either may run to completion before
//! the other starts, and an even split of winners across many rounds is exactly
//! what that produces, so it is not evidence of overlap either. A test that
//! announces "about to call" from its own code proves only that a thread reached
//! the test's own line, not that it reached the lock.
//!
//! What is observable is the boundary itself, from inside. Two reports are
//! enough, and they are deliberately on opposite sides of the acquisition:
//!
//! - [`reaching`] fires before the lock is taken, so a caller that has reported
//!   it is inside `lock_ledger` with nothing left to do but acquire;
//! - [`inside`] fires after it is taken, so a caller that has reported it holds
//!   the guard, and one that has *not* has provably not entered the critical
//!   section.
//!
//! Together they let a test establish the scenario mechanically: hold one caller
//! inside, watch the other reach the boundary and fail to enter, then let the
//! first go.
//!
//! **Scoped to threads a particular harness registered, and that scoping is
//! load-bearing rather than tidy.** Both reports are inert unless the calling
//! thread carries the token [`Boundary::spawn`] sets, so every other caller of
//! `lock_ledger` passes through untouched. Without it the seam would reach
//! whichever thread happened to arrive first, and the consequence is not a wrong
//! reading but a stall: an unrelated test's ledger call would become the caller
//! held inside the critical section, hanging both it and this one. Scoping by
//! *process* would not fix that — the runner's execution policy is a
//! convenience, not a safety boundary, and a harness that runs tests
//! concurrently in one process is a perfectly ordinary thing to point at this
//! crate.
//!
//! The token names *which* harness, and that is not decoration. A contender
//! outlives the boundary's hold: a harness releases the caller it was holding
//! and only then joins, so its threads are still running ledger operations
//! afterwards. With a bare flag one of those stragglers reports into — and can
//! be held by — whichever harness armed next, which is the original hazard one
//! level down, and it shows up as the wrong caller being held rather than as
//! anything that looks like a scoping bug.
//!
//! Two harnesses cannot observe at once either: [`watch`] holds an exclusive
//! guard for the whole life of its [`Boundary`], which deliberately outlasts
//! [`Boundary::release`] — releasing the held caller ends the hold, not the
//! observation, and the guard has to survive until the contenders are joined.
//!
//! **One constraint on use.** While a caller is held inside, it owns the ledger
//! guard, so the harness thread must not touch the ledger until it releases —
//! a fixture read taken in that window would wait on the very caller the
//! harness is about to let go.
//!
//! Deliberately not a `Transport` fake or an injectable executor. Those would be
//! production-shaped: a new arm on a real enum, widened visibility, a second
//! consumer of an interface that has only one. This is none of those. It is
//! absent from the shipped binary, names no production type, and the whole of
//! its production footprint is two calls that compile to nothing.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long a caller is given to prove it can enter the critical section.
///
/// A bound on how long a violation has to appear rather than a race the test
/// must win: a caller able to enter finishes in microseconds. Longer is
/// strictly stricter.
const ENTRY_WINDOW: Duration = Duration::from_millis(100);

/// How long the harness waits for a report that must arrive. Generous, because
/// nothing turns on its value — a caller that has not reported within it has
/// died, and the wait exists so that failure is a message rather than a hang.
const REPORT_WINDOW: Duration = Duration::from_secs(5);

thread_local! {
    /// Which harness registered this thread, if any.
    ///
    /// Set on the contender's own thread as its first act, so no thread the
    /// harness did not spawn can carry it — which is what makes the scoping a
    /// property of the seam rather than a convention its callers must keep.
    ///
    /// It names a *particular* harness rather than merely recording that some
    /// harness registered the thread, because a contender outlives the boundary
    /// that spawned it: a harness releases the held caller and only then joins,
    /// so its threads are still running ledger operations after it has finished
    /// observing. A bare flag lets one of those stragglers report into — and be
    /// held by — whichever harness armed next, which is the same hazard as an
    /// unregistered thread being seen, one level down.
    static CONTENDING: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Only one harness observes at a time. Held for the whole life of a
/// [`Boundary`], which deliberately outlasts the release of its held caller: the
/// contenders are still running until the harness joins them, and a boundary
/// that stood down at release would free this while they were.
static EXCLUSIVE: Mutex<()> = Mutex::new(());

static WATCH: Mutex<Option<Reports>> = Mutex::new(None);

/// Distinguishes one harness from the next, so a straggler cannot report into a
/// boundary that did not spawn it.
static NEXT_HARNESS: AtomicU64 = AtomicU64::new(1);

/// Where a registered caller reports, and what holds the first one that enters.
struct Reports {
    harness: u64,
    reaching: Sender<()>,
    inside: Sender<()>,
    /// Taken by the first registered caller to enter the critical section, which
    /// then blocks on it. `None` afterwards, so later callers pass straight
    /// through: exactly one is held, and it is the one the harness awaits.
    hold: Option<Receiver<()>>,
}

/// The harness this thread contends for, if it is a contender at all.
fn contending_for() -> Option<u64> {
    CONTENDING.with(Cell::get)
}

/// Whether `reports` belongs to the harness this thread was spawned by.
fn ours(reports: &Reports) -> bool {
    contending_for() == Some(reports.harness)
}

/// Reports a registered caller arriving at the boundary, before the lock is
/// taken.
pub(super) fn reaching() {
    if contending_for().is_none() {
        return;
    }
    let watch = WATCH.lock().unwrap_or_else(|held| held.into_inner());
    if let Some(reports) = watch.as_ref().filter(|reports| ours(reports)) {
        let _ = reports.reaching.send(());
    }
}

/// Reports a registered caller entering the critical section, and holds the
/// first one there until the harness releases it.
///
/// The ledger guard is held throughout, which is the point: everything else that
/// wants the ledger is genuinely blocked meanwhile. This module's own mutex is
/// released before blocking, so the harness can still reach it.
pub(super) fn inside() {
    if contending_for().is_none() {
        return;
    }
    let hold = {
        let mut watch = WATCH.lock().unwrap_or_else(|held| held.into_inner());
        let Some(reports) = watch.as_mut().filter(|reports| ours(reports)) else {
            return;
        };
        let _ = reports.inside.send(());
        reports.hold.take()
    };
    if let Some(hold) = hold {
        let _ = hold.recv();
    }
}

/// The harness's end of the boundary.
///
/// Disarms on drop, so a test that fails partway does not leave the boundary
/// holding a caller inside the ledger's critical section.
pub(in crate::relay::delivery::admission) struct Boundary {
    harness: u64,
    reaching: Receiver<()>,
    inside: Receiver<()>,
    release: Sender<()>,
    _exclusive: MutexGuard<'static, ()>,
}

/// Begins observing. Only threads started by [`Boundary::spawn`] report; the
/// first of them to enter the critical section is held there.
///
/// Observing ends when the returned value is dropped, which must be after the
/// harness has joined its contenders — releasing the held caller is not the end
/// of the boundary's job, only of its hold.
pub(in crate::relay::delivery::admission) fn watch() -> Boundary {
    let exclusive = EXCLUSIVE.lock().unwrap_or_else(|held| held.into_inner());
    let harness = NEXT_HARNESS.fetch_add(1, Ordering::Relaxed);
    let (reaching_report, reaching) = channel();
    let (inside_report, inside) = channel();
    let (release, hold) = channel();
    *WATCH.lock().unwrap_or_else(|held| held.into_inner()) = Some(Reports {
        harness,
        reaching: reaching_report,
        inside: inside_report,
        hold: Some(hold),
    });
    Boundary {
        harness,
        reaching,
        inside,
        release,
        _exclusive: exclusive,
    }
}

impl Boundary {
    /// Runs `operation` on a thread this boundary observes.
    ///
    /// The registration is the thread's own first act rather than something the
    /// caller passes in, so a contender cannot be half-registered and no thread
    /// started any other way is ever seen here.
    pub(in crate::relay::delivery::admission) fn spawn<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> JoinHandle<T> {
        let harness = self.harness;
        std::thread::spawn(move || {
            CONTENDING.with(|contending| contending.set(Some(harness)));
            operation()
        })
    }

    /// Blocks until a registered caller is inside the critical section, holding
    /// the ledger guard, and consumes the boundary report it made on the way in.
    ///
    /// After this returns, that caller holds the lock and will keep holding it
    /// until [`release`](Self::release). That is the scenario's premise
    /// established as fact rather than arranged and hoped for.
    pub(in crate::relay::delivery::admission) fn await_holder(&self) {
        self.inside
            .recv_timeout(REPORT_WINDOW)
            .expect("a registered caller enters the critical section and is held there");
        self.reaching
            .recv_timeout(REPORT_WINDOW)
            .expect("the held caller reached the boundary before it entered");
    }

    /// Blocks until another registered caller reaches the boundary.
    ///
    /// It is then inside `lock_ledger` with nothing left but the acquisition —
    /// which is what separates this from a test-side announcement, where a
    /// thread might not have entered the operation at all.
    pub(in crate::relay::delivery::admission) fn await_arrival(&self) {
        self.reaching
            .recv_timeout(REPORT_WINDOW)
            .expect("the second registered caller reaches the ledger lock");
    }

    /// Asserts that nobody else enters the critical section while the held
    /// caller occupies it.
    ///
    /// Falsifiable in the direction that matters: a caller able to act without
    /// the lock enters and fails this. A caller that is merely slow leaves it
    /// passing, having proved less.
    pub(in crate::relay::delivery::admission) fn assert_none_entered(&self) {
        assert_eq!(
            self.inside.recv_timeout(ENTRY_WINDOW).err(),
            Some(RecvTimeoutError::Timeout),
            "no second caller may enter the critical section while one is held inside it"
        );
    }

    /// Lets the held caller finish.
    ///
    /// Takes `&self` rather than consuming the boundary, which is load-bearing:
    /// the contenders are still running until the harness joins them, and a
    /// boundary that stood down here would free the exclusive guard while its
    /// own stragglers were still calling into the ledger.
    pub(in crate::relay::delivery::admission) fn release(&self) {
        let _ = self.release.send(());
    }
}

impl Drop for Boundary {
    fn drop(&mut self) {
        let _ = self.release.send(());
        *WATCH.lock().unwrap_or_else(|held| held.into_inner()) = None;
    }
}
