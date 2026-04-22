use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{LookSnapshotPayload, RelayRequest, RelayResponse, handle_request},
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

fn tmux_available() -> bool {
    StdCommand::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn tmux_command(socket: &Path, arguments: &[&str]) -> std::process::Output {
    StdCommand::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run tmux command")
}

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_root, bundle_name, runtime_directory)
}

struct TmuxServerGuard {
    socket: PathBuf,
}

impl TmuxServerGuard {
    fn new(socket: PathBuf) -> Self {
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

fn wait_for_pane_contains(socket: &Path, target: &str, needle: &str, timeout: Duration) {
    let started = Instant::now();
    loop {
        let output = tmux_command(socket, &["capture-pane", "-p", "-t", target, "-S", "-40"]);
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

fn write_bundle_configuration(root: &Path, bundle_name: &str, sessions: &[&str]) -> PathBuf {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "all:home"
send = "all:home"
"#,
    )
    .expect("write policies file");
    let mut bundle_toml = String::from("format-version = 1\n");
    for session in sessions {
        bundle_toml.push_str(
            format!(
                "\n[[sessions]]\nid = \"{session}\"\nname = \"{session}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n"
            )
            .as_str(),
        );
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    config_root
}

fn spawn_session(socket: &Path, session_name: &str, shell_command: &str) {
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

#[test]
fn relay_look_returns_oldest_to_newest_snapshot_lines() {
    if !tmux_available() {
        eprintln!("skipping relay look test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(
        &paths.tmux_socket,
        "alpha",
        "printf 'alpha-ready\\n'; exec sleep 45",
    );
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'LOOK-A\\nLOOK-B\\nLOOK-C\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "LOOK-C",
        Duration::from_secs(3),
    );

    let response = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(3),
            bundle_name: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("look response");
    let RelayResponse::Look {
        snapshot: LookSnapshotPayload::Lines { snapshot_lines },
        ..
    } = response
    else {
        panic!("expected look response");
    };
    assert_eq!(snapshot_lines, vec!["LOOK-A", "LOOK-B", "LOOK-C"]);
}

#[test]
fn relay_look_allows_optional_or_matching_bundle_and_applies_default_lines() {
    if !tmux_available() {
        eprintln!("skipping relay look test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_configuration(temporary.path(), bundle_name, &["alpha", "bravo"]);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    spawn_session(
        &paths.tmux_socket,
        "alpha",
        "printf 'alpha-ready\\n'; exec sleep 45",
    );
    spawn_session(
        &paths.tmux_socket,
        "bravo",
        "printf 'LOOK-1\\nLOOK-2\\nLOOK-3\\n'; exec sleep 45",
    );
    wait_for_pane_contains(
        &paths.tmux_socket,
        "bravo",
        "LOOK-3",
        Duration::from_secs(3),
    );

    let omitted_bundle = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: None,
            bundle_name: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("look response");
    let RelayResponse::Look {
        snapshot: LookSnapshotPayload::Lines {
            snapshot_lines: omitted_lines,
        },
        ..
    } = omitted_bundle
    else {
        panic!("expected look response");
    };

    let matching_bundle = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: None,
            bundle_name: Some(bundle_name.to_string()),
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("look response");
    let RelayResponse::Look {
        snapshot: LookSnapshotPayload::Lines {
            snapshot_lines: matching_lines,
        },
        ..
    } = matching_bundle
    else {
        panic!("expected look response");
    };

    let explicit_default = dispatch_request(
        RelayRequest::Look {
            requester_session: "alpha".to_string(),
            target_session: "bravo".to_string(),
            lines: Some(120),
            bundle_name: None,
        },
        &config_root,
        bundle_name,
        &paths.runtime_directory,
    )
    .expect("look response");
    let RelayResponse::Look {
        snapshot:
            LookSnapshotPayload::Lines {
                snapshot_lines: explicit_default_lines,
            },
        ..
    } = explicit_default
    else {
        panic!("expected look response");
    };

    assert_eq!(omitted_lines, matching_lines);
    assert_eq!(omitted_lines, explicit_default_lines);
}
