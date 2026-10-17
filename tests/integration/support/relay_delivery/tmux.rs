use std::{
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::{Duration, Instant},
};

pub(crate) fn tmux_available() -> bool {
    StdCommand::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) fn tmux_command(
    socket: &Path,
    arguments: &[&str],
) -> std::process::Output {
    StdCommand::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run tmux command")
}

pub(crate) struct TmuxServerGuard {
    socket: PathBuf,
}

impl TmuxServerGuard {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl Drop for TmuxServerGuard {
    fn drop(&mut self) {
        let _ = StdCommand::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-server"])
            .output();
    }
}

pub(crate) fn wait_for_pane_contains(
    socket: &Path,
    target: &str,
    needle: &str,
    timeout: Duration,
) {
    let started = Instant::now();
    loop {
        let output = tmux_command(
            socket,
            &["capture-pane", "-p", "-t", target, "-S", "-40"],
        );
        assert!(
            output.status.success(),
            "failed to capture pane for {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let snapshot = String::from_utf8_lossy(&output.stdout);
        if snapshot.contains(needle) {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for '{needle}' in {target} pane, snapshot={snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn capture_pane(
    socket: &Path,
    target: &str,
    lines: &str,
) -> String {
    let output = tmux_command(
        socket,
        &["capture-pane", "-p", "-t", target, "-S", lines],
    );
    assert!(
        output.status.success(),
        "failed to capture pane for {target}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn spawn_session(
    socket: &Path,
    session_name: &str,
    shell_command: &str,
) {
    let output = tmux_command(
        socket,
        &[
            "new-session",
            "-d",
            "-s",
            session_name,
            "sh",
            "-lc",
            shell_command,
        ],
    );
    assert!(
        output.status.success(),
        "failed to create session {session_name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
