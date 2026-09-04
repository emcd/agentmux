//! Pty startup lifecycle: guard, bounded cleanup, and inner bootstrap.
//!
//! Extracted from `transport.rs` as a mechanical split — no behavior
//! change. The bring-up owns `StartupGuard` (partial child/thread
//! ownership + bounded `observe_thread_finished`) and
//! `PtyTransport::startup_inner` (the `openpty` → `spawn_command` →
//! worker/reader thread launch that `Transport::startup` publishes around).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc;

use crate::pty::command::program_and_args;
use crate::transports::{TransportError, TransportReadiness, TransportStatus};

use super::{PtyChildHandle, PtyTransport, STARTUP_CLEANUP_JOIN_BOUND, STARTUP_CLEANUP_POLL};

impl PtyTransport {
    /// Performs the bootstrap work behind [`crate::transports::Transport::startup`].
    /// Opens the PTY pair, spawns the child, launches the worker and
    /// reader threads, and stashes the runtime handles.
    pub(crate) fn startup_inner(
        &mut self,
        context: &crate::transports::StartupContext,
    ) -> Result<TransportStatus, TransportError> {
        let mut guard = StartupGuard::new(self.shutdown_flag.clone());

        let cols = self.shared.config.cols;
        let rows = self.shared.config.rows;
        let initial_command = if self.configured_initial_command.is_empty() {
            "/bin/bash".to_string()
        } else {
            self.configured_initial_command.clone()
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TransportError {
                code: "pty_open_failed".to_string(),
                reason: format!("portable_pty::openpty: {e}"),
                details: None,
            })?;

        let (program, args) = program_and_args(&initial_command).map_err(|e| TransportError {
            code: "pty_command_parse_failed".to_string(),
            reason: format!("tokenize initial_command {initial_command:?}: {e}"),
            details: None,
        })?;
        let mut cmd = CommandBuilder::new(&program);
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.env("TERM", self.configured_term_protocol.as_env_var());
        cmd.env("COLORTERM", "truecolor");
        for entry in &self.target_member.environment {
            cmd.env(entry.name.as_str(), entry.value.as_str());
        }
        let cwd = self
            .configured_working_directory
            .as_deref()
            .or(Some(context.runtime_directory.as_path()));
        if let Some(cwd) = cwd.and_then(|p| p.to_str()) {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| TransportError {
            code: "pty_spawn_failed".to_string(),
            reason: format!("spawn_command: {e}"),
            details: None,
        })?;
        drop(pair.slave);

        let child_arc: PtyChildHandle = Arc::new(std::sync::Mutex::new(child));
        guard.note_child(child_arc.clone());

        let reader = pair.master.try_clone_reader().map_err(|e| TransportError {
            code: "pty_reader_clone_failed".to_string(),
            reason: format!("try_clone_reader: {e}"),
            details: None,
        })?;
        let writer = pair.master.take_writer().map_err(|e| TransportError {
            code: "pty_writer_take_failed".to_string(),
            reason: format!("take_writer: {e}"),
            details: None,
        })?;
        let writer_arc: Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>> =
            Arc::new(std::sync::Mutex::new(writer));

        let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>(256);
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<crate::pty::state::SnapshotRequest>(64);

        self.shared.snapshot_tx = snapshot_tx.clone();
        let shared_for_worker = crate::pty::state::PtyShared {
            config: self.shared.config.clone(),
            snapshot_tx,
            child_exited: self.shared.child_exited.clone(),
        };

        let target_session = self.target_member.id.clone();
        let target_session_for_worker = target_session.clone();
        let writer_for_worker = writer_arc.clone();
        let child_for_worker = child_arc.clone();
        let shutdown_flag_for_worker = self.shutdown_flag.clone();
        let mirror_state_for_worker = self.mirror_state.clone();
        let delivery_for_worker = self.delivery.clone();
        let readiness_for_worker = self.readiness.clone();
        // The same latch the transport's own `health` folds into. One latch and
        // one clock: a writer with its own would restart `since` on every poll,
        // and a dwell measured from a `since` that keeps moving never elapses.
        let unreachable_for_worker = Arc::clone(&self.unreachable_since);

        let bytes_tx_for_reader = bytes_tx.clone();
        let reader_shutdown_flag = self.shutdown_flag.clone();
        let child_exited_for_reader = self.shared.child_exited.clone();

        let worker_handle = thread::Builder::new()
            .name(format!("pty-worker-{target_session_for_worker}"))
            .spawn(move || {
                super::runtime::run_worker(
                    cols,
                    rows,
                    bytes_rx,
                    snapshot_rx,
                    shared_for_worker,
                    writer_for_worker,
                    child_for_worker,
                    target_session_for_worker,
                    shutdown_flag_for_worker,
                    mirror_state_for_worker,
                    readiness_for_worker,
                    unreachable_for_worker,
                    delivery_for_worker,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_worker_spawn_failed".to_string(),
                reason: format!("worker thread spawn: {e}"),
                details: None,
            })?;
        guard.note_worker(worker_handle);

        let reader_handle = thread::Builder::new()
            .name(format!("pty-reader-{target_session}"))
            .spawn(move || {
                super::runtime::run_reader(
                    reader,
                    bytes_tx_for_reader,
                    reader_shutdown_flag,
                    child_exited_for_reader,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_reader_spawn_failed".to_string(),
                reason: format!("reader thread spawn: {e}"),
                details: None,
            })?;
        guard.note_reader(reader_handle);

        let (child, worker, reader) = guard.finish();
        self.bytes_tx = Some(bytes_tx);
        self.child = Some(child);
        self.worker_handle = Some(worker);
        self.reader_handle = Some(reader);

        Ok(TransportStatus {
            readiness: TransportReadiness::Pending,
        })
    }
}

/// Explicit ownership guard for partial startup resources.
struct StartupGuard {
    shutdown_flag: Arc<AtomicBool>,
    child: Option<PtyChildHandle>,
    worker_handle: Option<thread::JoinHandle<()>>,
    reader_handle: Option<thread::JoinHandle<()>>,
    disarmed: bool,
}

impl StartupGuard {
    fn new(shutdown_flag: Arc<AtomicBool>) -> Self {
        Self {
            shutdown_flag,
            child: None,
            worker_handle: None,
            reader_handle: None,
            disarmed: false,
        }
    }

    fn note_child(&mut self, child: PtyChildHandle) {
        self.child = Some(child);
    }

    fn note_worker(&mut self, handle: thread::JoinHandle<()>) {
        self.worker_handle = Some(handle);
    }

    fn note_reader(&mut self, handle: thread::JoinHandle<()>) {
        self.reader_handle = Some(handle);
    }

    fn finish(
        mut self,
    ) -> (
        PtyChildHandle,
        thread::JoinHandle<()>,
        thread::JoinHandle<()>,
    ) {
        self.disarmed = true;
        let child = self
            .child
            .take()
            .expect("StartupGuard invariant: child is set before finish");
        let worker = self
            .worker_handle
            .take()
            .expect("StartupGuard invariant: worker_handle is set before finish");
        let reader = self
            .reader_handle
            .take()
            .expect("StartupGuard invariant: reader_handle is set before finish");
        (child, worker, reader)
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        self.shutdown_flag.store(true, Ordering::Release);

        let child_arc = self.child.take();
        if let Some(child_arc) = child_arc {
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.kill();
            }
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.wait();
            }
        }

        if let Some(handle) = self.reader_handle.take()
            && !observe_thread_finished(handle, STARTUP_CLEANUP_JOIN_BOUND)
        {
            eprintln!(
                "[pty] startup cleanup could not observe the reader thread finish within \
                 {STARTUP_CLEANUP_JOIN_BOUND:?}; leaving it to exit on its own"
            );
        }
        if let Some(handle) = self.worker_handle.take()
            && !observe_thread_finished(handle, STARTUP_CLEANUP_JOIN_BOUND)
        {
            eprintln!(
                "[pty] startup cleanup could not observe the worker thread finish within \
                 {STARTUP_CLEANUP_JOIN_BOUND:?}; it may be stalled in Terminal::new and is \
                 left running to exit on its own"
            );
        }
    }
}

/// Boundedly observes a thread's completion instead of blocking on it.
fn observe_thread_finished(handle: thread::JoinHandle<()>, bound: Duration) -> bool {
    let deadline = std::time::Instant::now() + bound;
    while !handle.is_finished() && std::time::Instant::now() < deadline {
        thread::sleep(STARTUP_CLEANUP_POLL);
    }
    if handle.is_finished() {
        let _ = handle.join();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn startup_cleanup_observes_thread_within_bound_and_detaches_beyond_it() {
        let quick = thread::spawn(|| {});
        while !quick.is_finished() {
            thread::yield_now();
        }
        assert!(
            observe_thread_finished(quick, Duration::from_secs(2)),
            "a finished thread should be observed and joined",
        );

        let slow = thread::spawn(|| thread::sleep(Duration::from_millis(100)));
        assert!(
            !observe_thread_finished(slow, Duration::from_millis(10)),
            "a thread not finished by the bound should be detached, not joined",
        );
    }
}
