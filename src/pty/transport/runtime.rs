//! Pty worker/reader threads — the `!Send` terminal lives here.
//!
//! The worker thread constructs the `libghostty-vt` terminal, installs its
//! callbacks, and then hands it to
//! [`PtyDeliveryWriter`](crate::pty::delivery::PtyDeliveryWriter) and runs the
//! shared delivery-loop executor for the rest of the generation's life. That is
//! the whole of this thread's job: the terminal cannot leave it, so the delivery
//! loop has to come to the terminal rather than the other way round.
//!
//! The reader owns the blocking PTY master read and feeds its bytes across to
//! the worker.

use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use tokio::sync::mpsc;

use crate::pty::delivery::PtyDeliveryWriter;
use crate::pty::state::{PtyShared, SnapshotRequest};
use crate::transports::{
    DeliveryExecutorContext, UnreachableSince, WorkerReadinessState, run_delivery_executor,
};

use super::{PTY_VERSION_STRING, PtyMirrorStateFn, READER_IDLE_POLL};

#[allow(clippy::too_many_arguments)]
pub(super) fn run_worker(
    cols: u16,
    rows: u16,
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    shared: PtyShared,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    target_session: String,
    shutdown_flag: Arc<AtomicBool>,
    mirror_state: Option<PtyMirrorStateFn>,
    readiness: Arc<Mutex<WorkerReadinessState>>,
    unreachable_since: Arc<UnreachableSince>,
    delivery: DeliveryExecutorContext,
) {
    let mut terminal = match libghostty_vt::Terminal::new(libghostty_vt::TerminalOptions {
        cols,
        rows,
        max_scrollback: 10_000,
    }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[pty-worker-{target_session}] Terminal::new failed: {e}");
            publish(
                &readiness,
                mirror_state.as_ref(),
                WorkerReadinessState::Unavailable,
            );
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
            return;
        }
    };

    install_handlers(&mut terminal, writer.clone(), cols, rows);

    if shutdown_flag.load(Ordering::Acquire) {
        return;
    }

    publish(
        &readiness,
        mirror_state.as_ref(),
        WorkerReadinessState::Available,
    );

    run_delivery_executor(
        PtyDeliveryWriter::new(
            terminal,
            bytes_rx,
            snapshot_rx,
            writer,
            shared,
            Arc::clone(&readiness),
            mirror_state.clone(),
            unreachable_since,
            shutdown_flag,
        ),
        delivery,
    );

    // The executor has returned, so nothing on this side will service the
    // terminal or take another write. Published here rather than left to
    // whichever event ended the loop, because the two that can — a shutdown
    // request and the child departing — would otherwise agree on the state only
    // by coincidence.
    publish(
        &readiness,
        mirror_state.as_ref(),
        WorkerReadinessState::Unavailable,
    );
}

pub(super) fn publish(
    readiness: &Arc<Mutex<WorkerReadinessState>>,
    mirror_state: Option<&PtyMirrorStateFn>,
    state: WorkerReadinessState,
) {
    *readiness.lock().expect("pty readiness mutex") = state;
    if let Some(mirror) = mirror_state {
        mirror(state);
    }
}

fn install_handlers(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    cols: u16,
    rows: u16,
) {
    let writer_for_pty_write = writer.clone();
    terminal
        .on_pty_write(move |_t, data| {
            if let Ok(mut g) = writer_for_pty_write.lock() {
                let _ = g.write_all(data);
            }
        })
        .expect("install on_pty_write callback");
    let _ = (writer, cols, rows);
    terminal
        .on_size(move |_t| {
            Some(libghostty_vt::terminal::SizeReportSize {
                rows,
                columns: cols,
                cell_width: 8,
                cell_height: 16,
            })
        })
        .expect("install on_size callback");
    terminal
        .on_device_attributes(|_t| {
            use libghostty_vt::terminal::{
                ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
                PrimaryDeviceAttributes, SecondaryDeviceAttributes,
            };
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[
                        DeviceAttributeFeature::COLUMNS_132,
                        DeviceAttributeFeature::SELECTIVE_ERASE,
                        DeviceAttributeFeature::ANSI_COLOR,
                    ],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 1,
                    rom_cartridge: 0,
                },
                tertiary: Default::default(),
            })
        })
        .expect("install on_device_attributes callback");
    terminal
        .on_xtversion(|_t| Some(PTY_VERSION_STRING))
        .expect("install on_xtversion callback");
    terminal
        .on_title_changed(|_t| {})
        .expect("install on_title_changed callback");
}

pub(super) fn run_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    bytes_tx: mpsc::Sender<Vec<u8>>,
    shutdown_flag: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 4096];
    while !shutdown_flag.load(Ordering::Acquire) {
        match reader.read(&mut buf) {
            Ok(0) => {
                child_exited.store(true, Ordering::Release);
                break;
            }
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                if bytes_tx.blocking_send(bytes).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(READER_IDLE_POLL);
            }
            Err(_) => {
                child_exited.store(true, Ordering::Release);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::mpsc;

    struct FatalErrReader;

    impl Read for FatalErrReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("simulated fatal read"))
        }
    }

    /// A fatal (non-`WouldBlock`) read error is the reader's only way to learn
    /// the child is gone without an EOF, and latching `child_exited` is what
    /// carries that finding to the executor's health axis. `run_reader` is
    /// crate-private and takes its reader by value, so the failing reader can
    /// only be injected from inside this module.
    #[test]
    fn reader_fatal_err_sets_child_exited() {
        let child_exited = Arc::new(AtomicBool::new(false));
        let (bytes_tx, _bytes_rx) = mpsc::channel::<Vec<u8>>(256);
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        run_reader(
            Box::new(FatalErrReader),
            bytes_tx,
            shutdown_flag,
            child_exited.clone(),
        );
        assert!(
            child_exited.load(Ordering::Acquire),
            "fatal reader Err (non-WouldBlock) should set child_exited",
        );
    }
}
