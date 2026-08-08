//! Unit coverage for Pty prompt readiness and look snapshots.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use agentmux::pty::{
    PtyConfigSnapshot, PtyOutputView, PtyPromptProbe, PtyShared, SnapshotResponse,
};
use agentmux::transports::{LookMode, LookSnapshotPayload, OutputView};
use regex::Regex;
use tokio::sync::mpsc;

fn shared_with(script: Vec<SnapshotResponse>) -> (PtyShared, thread::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<agentmux::pty::SnapshotRequest>(8);
    let script = Arc::new(Mutex::new(VecDeque::from(script)));
    let worker_script = Arc::clone(&script);
    let handle = thread::spawn(move || {
        while let Some(request) = rx.blocking_recv() {
            let response = worker_script
                .lock()
                .expect("script mutex")
                .pop_front()
                .unwrap_or(SnapshotResponse {
                    tail: String::new(),
                    cursor_x: 0,
                    cursor_y: 0,
                    cursor_visible: false,
                });
            let _ = request.tx.send(response);
        }
    });
    let shared = PtyShared {
        config: PtyConfigSnapshot {
            target_member_id: "test-session".to_string(),
            cols: 120,
            rows: 40,
            prompt_regex: Some(Regex::new(r"READY").expect("regex")),
            prompt_inspect_lines: 3,
            prompt_idle_column: Some(2),
        },
        snapshot_tx: tx,
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    (shared, handle)
}

#[test]
fn pty_prompt_probe_reports_ready_from_snapshot_and_cursor() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "READY".to_string(),
        cursor_x: 2,
        cursor_y: 0,
        cursor_visible: true,
    }]);
    let mut probe = PtyPromptProbe::new(shared);

    assert!(probe.observe().expect("snapshot observation"));
    drop(probe);
    handle.join().expect("snapshot worker");
}

#[test]
fn pty_prompt_probe_rejects_cursor_mismatch() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "READY".to_string(),
        cursor_x: 3,
        cursor_y: 0,
        cursor_visible: true,
    }]);
    let mut probe = PtyPromptProbe::new(shared);

    assert!(!probe.observe().expect("snapshot observation"));
    drop(probe);
    handle.join().expect("snapshot worker");
}

#[test]
fn pty_look_returns_requested_tail() {
    let (shared, handle) = shared_with(vec![SnapshotResponse {
        tail: "one\ntwo\nthree".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }]);
    let view = PtyOutputView::new(shared);

    let result = view
        .look(LookMode {
            lines: Some(2),
            offset: None,
            prime_timeout: Duration::ZERO,
        })
        .expect("look snapshot");
    assert!(matches!(
        result,
        LookSnapshotPayload::Lines { snapshot_lines }
            if snapshot_lines == vec!["two".to_string(), "three".to_string()]
    ));
    drop(view);
    handle.join().expect("snapshot worker");
}
