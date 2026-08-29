//! Pty worker/reader runtime — the `!Send` terminal lives here.
//!
//! Extracted from `transport.rs` as a mechanical split — no behavior
//! change. The worker owns the `libghostty-vt` terminal (thread-local),
//! the delivery `Delivery` state machine, and the snapshot/bytes/write
//! channel pumps. The reader owns the blocking PTY master read.

use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use tokio::sync::mpsc;

use crate::pty::delivery::{Delivery, DeliveryStep, PendingRaw};
use crate::pty::state::{PtyShared, SnapshotRequest, SnapshotResponse};
use crate::transports::{SingleDeliveryOutcome, WorkerReadinessState};

use super::{PTY_VERSION_STRING, PtyMirrorStateFn, READER_IDLE_POLL, WORKER_IDLE_POLL};

#[allow(clippy::too_many_arguments)]
pub(super) fn run_worker(
    cols: u16,
    rows: u16,
    mut bytes_rx: mpsc::Receiver<Vec<u8>>,
    mut write_rx: mpsc::Receiver<super::DeliveryCommand>,
    mut snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    shared: PtyShared,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    target_session: String,
    shutdown_flag: Arc<AtomicBool>,
    mirror_state: Option<PtyMirrorStateFn>,
    readiness: Arc<Mutex<WorkerReadinessState>>,
    partition_sink: Arc<dyn crate::transports::PartitionSink>,
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

    let mut delivery: Option<Delivery> = None;
    let mut pending_raw: Option<PendingRaw> = None;

    while !shutdown_flag.load(Ordering::Acquire) {
        if shared.child_exited.load(Ordering::Acquire) {
            publish(
                &readiness,
                mirror_state.as_ref(),
                WorkerReadinessState::Unavailable,
            );
            resolve_pending_raw_failed(&mut pending_raw, &target_session);
            abandon_in_flight(&mut delivery, &target_session);
            drain_remaining_with_failed(&mut write_rx, &target_session);
            break;
        }

        while let Ok(request) = snapshot_rx.try_recv() {
            handle_snapshot(&mut terminal, request);
        }

        if let Some(raw) = pending_raw.take() {
            match Delivery::start_raw(
                raw.content,
                raw.append_enter,
                raw.outcome_tx,
                &writer,
                &target_session,
            ) {
                Ok(d) => {
                    publish(
                        &readiness,
                        mirror_state.as_ref(),
                        WorkerReadinessState::Busy,
                    );
                    delivery = Some(d);
                    continue;
                }
                Err(_) => {
                    continue;
                }
            }
        }

        if let Some(d) = delivery.as_mut() {
            match d.step(&target_session) {
                DeliveryStep::Done {
                    pending_raw: next_raw,
                } => {
                    publish(
                        &readiness,
                        mirror_state.as_ref(),
                        WorkerReadinessState::Available,
                    );
                    delivery = None;
                    pending_raw = next_raw;
                    continue;
                }
            }
        }

        while let Ok(bytes) = bytes_rx.try_recv() {
            terminal.vt_write(&bytes);
        }

        match write_rx.try_recv() {
            Ok(super::DeliveryCommand::Envelope {
                envelope,
                outcome_tx,
            }) => {
                let d = Delivery::start_envelope_group(
                    envelope,
                    outcome_tx,
                    &mut write_rx,
                    &writer,
                    &target_session,
                    partition_sink.as_ref(),
                );
                publish(
                    &readiness,
                    mirror_state.as_ref(),
                    WorkerReadinessState::Busy,
                );
                delivery = Some(d);
                continue;
            }
            Ok(super::DeliveryCommand::Raw {
                content,
                append_enter,
                outcome_tx,
            }) => {
                pending_raw = Some(PendingRaw {
                    content,
                    append_enter,
                    outcome_tx,
                });
                continue;
            }
            Err(_) => {
                thread::sleep(WORKER_IDLE_POLL);
            }
        }
    }

    drain_remaining(&mut write_rx, &target_session);
    while let Ok(req) = snapshot_rx.try_recv() {
        let _ = req.tx.send(SnapshotResponse {
            tail: String::new(),
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: false,
        });
    }
}

fn abandon_in_flight(delivery: &mut Option<Delivery>, target_session: &str) {
    if let Some(mut d) = delivery.take() {
        d.abandon_into_failed(
            target_session,
            "pty_child_exited",
            "pty child exited before delivery resolved",
        );
    }
}

fn drain_remaining_with_failed(
    write_rx: &mut mpsc::Receiver<super::DeliveryCommand>,
    target_session: &str,
) {
    while let Ok(cmd) = write_rx.try_recv() {
        let outcome_tx = match cmd {
            super::DeliveryCommand::Envelope { outcome_tx, .. } => outcome_tx,
            super::DeliveryCommand::Raw { outcome_tx, .. } => outcome_tx,
        };
        let _ = outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_child_exited".to_string()),
            reason: Some(
                "pty child exited before the queued delivery could be processed".to_string(),
            ),
            details: None,
        });
    }
}

fn resolve_pending_raw_failed(pending_raw: &mut Option<PendingRaw>, target_session: &str) {
    if let Some(raw) = pending_raw.take() {
        let _ = raw.outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_child_exited".to_string()),
            reason: Some("pty child exited before the pending raw could be processed".to_string()),
            details: None,
        });
    }
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

fn handle_snapshot(terminal: &mut libghostty_vt::Terminal<'_, '_>, request: SnapshotRequest) {
    let response = render_snapshot(terminal, request.inspect_lines);
    let _ = request.tx.send(response);
}

fn render_snapshot(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    inspect_lines: Option<usize>,
) -> SnapshotResponse {
    let formatter_result = libghostty_vt::fmt::Formatter::new(
        terminal,
        libghostty_vt::fmt::FormatterOptions::new()
            .with_format(libghostty_vt::fmt::Format::Plain)
            .with_trim(true),
    );
    let bytes = match formatter_result {
        Ok(formatter) => {
            let mut f = formatter;
            match f.format_alloc(None) {
                Ok(bytes) => bytes.as_ref().to_vec(),
                Err(_) => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    };
    let tail = String::from_utf8_lossy(&bytes).to_string();
    let lines_to_take = inspect_lines.unwrap_or(crate::pty::state::LOOK_LINES_DEFAULT);
    let mut collected: Vec<String> = tail
        .lines()
        .rev()
        .take(lines_to_take)
        .map(str::to_string)
        .collect();
    collected.reverse();
    let trimmed_tail = collected.join("\n");
    let cursor_x = terminal.cursor_x().unwrap_or(0);
    let cursor_y = terminal.cursor_y().unwrap_or(0);
    let cursor_visible = terminal.is_cursor_visible().unwrap_or(false);
    SnapshotResponse {
        tail: trimmed_tail,
        cursor_x,
        cursor_y,
        cursor_visible,
    }
}

fn drain_remaining(write_rx: &mut mpsc::Receiver<super::DeliveryCommand>, _target_session: &str) {
    while write_rx.try_recv().is_ok() {}
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
    use crate::transports::SingleDeliveryOutcome;
    use std::io::{self, Read};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::{mpsc, oneshot};

    struct FatalErrReader;

    impl Read for FatalErrReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("simulated fatal read"))
        }
    }

    #[test]
    fn reader_fatal_err_sets_child_exited_and_pending_raw_resolves_failed() {
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

        let (tx, rx) = oneshot::channel::<SingleDeliveryOutcome>();
        let mut pending_raw = Some(crate::pty::delivery::PendingRaw {
            content: "x".to_string(),
            append_enter: false,
            outcome_tx: tx,
        });
        resolve_pending_raw_failed(&mut pending_raw, "test-session");
        assert!(pending_raw.is_none(), "pending_raw slot should be consumed",);
        let outcome = rx
            .blocking_recv()
            .expect("receiver should get the Failed outcome");
        assert_eq!(outcome.target_session, "test-session");
        assert!(
            matches!(outcome.outcome, crate::transports::SendOutcome::Failed),
            "expected Failed, got {:?}",
            outcome.outcome,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("pty_child_exited"),
            "expected reason_code pty_child_exited, got {:?}",
            outcome.reason_code,
        );
    }
}
