//! Process-wide signal helpers for graceful relay shutdown.
//!
//! Shutdown carries a **deadline**, not a collection of independent durations.
//! Every bounded step on the shutdown path — draining connection workers,
//! waiting for delivery workers, fencing a generation — runs inside the same
//! watchdog grace that ends in a forced exit, so each one has to bound itself by
//! what is *left* rather than by a value configured in isolation. A duration
//! cannot express that relationship; a deadline can, and every nested step can
//! recompute its own share from it.
//!
//! This is not hypothetical tidiness. `fence-observation-timeout-ms` defaults to
//! 5s and is operator-configurable up to 60s, and the fence spends two of those
//! windows, inside a wait that gives it 1.5s, inside a grace that force-exits at
//! 5s. Work that depended on the fence finishing was simply lost when the
//! process exited underneath it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::error::RuntimeError;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
/// When the shutdown watchdog will force the process to exit.
///
/// Armed by whichever comes first: the watchdog observing the shutdown flag, or
/// the first step that needs a budget after the flag is set. Both are bounded by
/// the same registered grace, and the first arming wins, so the deadline that
/// stands is the earliest — which is the safe direction, since the watchdog's own
/// grace starts no earlier than the flag it is waiting to see.
///
/// It is deliberately *not* derived wherever cleanup happens to begin: that would
/// assume the grace had not already started, and hand out a deadline later than
/// the exit it is supposed to precede.
static SHUTDOWN_DEADLINE: Mutex<Option<Instant>> = Mutex::new(None);
/// The forced-exit grace this process runs under, registered when its shutdown
/// watchdog is spawned and before any signal can arrive.
///
/// Its presence is what distinguishes a process that *will* have a deadline from
/// one that never will. Without it, "no deadline yet" and "no deadline ever"
/// look identical, and the window between the signal handler setting the flag
/// and the watchdog observing it — a full poll interval — reads as the latter.
/// A budget computed in that window took its full configured bound and overran
/// the grace exactly as it did before any of this existed.
static SHUTDOWN_GRACE: Mutex<Option<Duration>> = Mutex::new(None);

extern "C" fn shutdown_signal_handler(_: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

#[inline]
fn shutdown_signal_handler_pointer() -> libc::sighandler_t {
    shutdown_signal_handler as *const () as libc::sighandler_t
}

/// Installed signal handlers that are restored on drop.
#[derive(Debug)]
pub struct ShutdownSignalGuard {
    previous_sigint: libc::sighandler_t,
    previous_sigterm: libc::sighandler_t,
}

impl Drop for ShutdownSignalGuard {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.previous_sigint);
            libc::signal(libc::SIGTERM, self.previous_sigterm);
        }
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        // Cleared with the flag it belongs to, so a process that installs
        // handlers more than once does not inherit an expired deadline and
        // conclude it has no time left before it has even been signalled.
        *SHUTDOWN_DEADLINE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *SHUTDOWN_GRACE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// Installs SIGINT/SIGTERM handlers that request graceful shutdown.
///
/// # Errors
///
/// Returns an I/O error if signal handlers cannot be installed.
pub fn install_shutdown_signal_handlers() -> Result<ShutdownSignalGuard, RuntimeError> {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    let previous_sigint = unsafe { libc::signal(libc::SIGINT, shutdown_signal_handler_pointer()) };
    if previous_sigint == libc::SIG_ERR {
        return Err(RuntimeError::io(
            "install SIGINT handler",
            std::io::Error::last_os_error(),
        ));
    }

    let previous_sigterm =
        unsafe { libc::signal(libc::SIGTERM, shutdown_signal_handler_pointer()) };
    if previous_sigterm == libc::SIG_ERR {
        unsafe {
            libc::signal(libc::SIGINT, previous_sigint);
        }
        return Err(RuntimeError::io(
            "install SIGTERM handler",
            std::io::Error::last_os_error(),
        ));
    }

    Ok(ShutdownSignalGuard {
        previous_sigint,
        previous_sigterm,
    })
}

/// Returns whether graceful shutdown has been requested.
#[must_use]
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Registers the forced-exit grace this process runs under.
///
/// Called once when the shutdown watchdog is spawned, before any signal can
/// arrive, so that a budget computed between the signal and the watchdog's
/// observation can establish the deadline itself rather than concluding there is
/// none.
pub fn register_shutdown_grace(grace: Duration) {
    *SHUTDOWN_GRACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(grace);
}

/// Arms the process-wide shutdown deadline at `grace` from now.
///
/// The first arming wins: a second call cannot extend the deadline, because the
/// watchdog that will actually force the exit is already counting down and a
/// later, longer deadline would be a promise nothing keeps.
pub fn arm_shutdown_deadline(grace: Duration) -> Instant {
    let mut deadline = SHUTDOWN_DEADLINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *deadline.get_or_insert_with(|| Instant::now() + grace)
}

/// Establishes the deadline on first need if shutdown is underway and this
/// process has a registered grace.
///
/// Arming here rather than waiting for the watchdog closes the window between
/// the handler setting the flag and the watchdog noticing. A deadline armed here
/// is **earlier** than the watchdog's, by however long the watchdog still has to
/// wait to observe — and earlier is the safe direction: every step finishes
/// before the forced exit rather than racing it. The watchdog's own later arming
/// is then a no-op, so the conservative deadline is the one that stands.
fn ensure_deadline_armed() {
    if !shutdown_requested() {
        return;
    }
    let grace = *SHUTDOWN_GRACE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(grace) = grace {
        arm_shutdown_deadline(grace);
    }
}

/// How long remains before the shutdown watchdog forces exit.
///
/// `None` means this process will never have a deadline — no watchdog registered
/// a grace, as in the CLI and test harnesses. A caller that gets `None` SHALL use
/// its own configured bound: there is nothing to fit inside, so nothing is being
/// violated by taking the full budget.
///
/// It does **not** mean "shutdown has not been observed yet". If shutdown is
/// underway and a grace is registered, this establishes the deadline rather than
/// reporting its absence — that gap was a live defect, not a documentation
/// nicety, because a budget taken inside it took its full configured bound.
///
/// `Some(Duration::ZERO)` means the grace has already elapsed. It is deliberately
/// not `None`: "no time left" and "no deadline" call for opposite behaviour, and
/// collapsing them would let an expired budget silently restore the full one.
#[must_use]
pub fn shutdown_time_remaining() -> Option<Duration> {
    ensure_deadline_armed();
    let deadline = SHUTDOWN_DEADLINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (*deadline).map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// Fits one bounded step inside the shutdown deadline.
///
/// Returns `configured` when no deadline is armed, and otherwise the smaller of
/// `configured` and whatever remains after setting `reserve` aside for the steps
/// that must still run once this one finishes. A step that consumed everything
/// left would meet its own bound and starve its successors, which is how the
/// original defect lost members: the fence finished nothing and the work behind
/// it never ran.
///
/// The result may be [`Duration::ZERO`] — the budget is already gone, and a
/// caller that treats zero as "skip the wait and go straight to the work behind
/// it" is behaving correctly rather than degrading.
#[must_use]
pub fn budget_within_shutdown(configured: Duration, reserve: Duration) -> Duration {
    match shutdown_time_remaining() {
        None => configured,
        Some(remaining) => configured.min(remaining.saturating_sub(reserve)),
    }
}
