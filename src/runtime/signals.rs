//! Process-wide signal helpers for graceful relay shutdown.

use std::sync::atomic::{AtomicBool, Ordering};

use super::error::RuntimeError;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

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
