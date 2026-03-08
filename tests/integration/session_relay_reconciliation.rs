use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use agentmux::{
    relay::reconcile_bundle,
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

#[derive(Clone)]
struct CoderSpec {
    id: String,
    initial_command: String,
    resume_command: String,
}

#[derive(Clone)]
struct SessionSpec {
    id: String,
    name: String,
    directory: PathBuf,
    coder: String,
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn tmux_command(socket: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run tmux command")
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
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-server"])
            .output();
    }
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for file {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    coders: &[CoderSpec],
    sessions: &[SessionSpec],
) -> PathBuf {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let mut coders_toml = String::from("format-version = 1\n");
    for coder in coders {
        coders_toml.push_str(
            format!(
                "\n[[coders]]\nid = \"{}\"\ninitial-command = \"{}\"\nresume-command = \"{}\"\n",
                coder.id, coder.initial_command, coder.resume_command
            )
            .as_str(),
        );
    }
    fs::write(config_root.join("coders.toml"), coders_toml).expect("write coders");

    let mut bundle_toml = String::from("format-version = 1\n");
    for session in sessions {
        bundle_toml.push_str(
            format!(
                "\n[[sessions]]\nid = \"{}\"\nname = \"{}\"\ndirectory = \"{}\"\ncoder = \"{}\"\n",
                session.id,
                session.name,
                session.directory.display(),
                session.coder
            )
            .as_str(),
        );
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    config_root
}

fn list_owned_sessions(socket: &Path) -> Vec<String> {
    let output = tmux_command(
        socket,
        &["list-sessions", "-F", "#{session_name}\t#{@agentmux_owned}"],
    );
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (name, marker) = line.split_once('\t').unwrap_or((line, ""));
            if marker.trim() == "1" {
                return Some(name.to_string());
            }
            None
        })
        .collect::<Vec<_>>()
}

#[test]
fn reconciliation_creates_missing_members_and_sets_owned_metadata() {
    if !tmux_available() {
        eprintln!("skipping reconciliation test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let alpha_directory = temporary.path().join("alpha");
    let bravo_directory = temporary.path().join("bravo");
    fs::create_dir_all(&alpha_directory).expect("create alpha directory");
    fs::create_dir_all(&bravo_directory).expect("create bravo directory");

    let bundle_name = "party";
    let config_root = write_bundle_configuration(
        temporary.path(),
        bundle_name,
        &[
            CoderSpec {
                id: "alpha-coder".to_string(),
                initial_command: "sh -lc 'printf alpha > alpha.started; exec sleep 45'".to_string(),
                resume_command: "sh -lc 'printf alpha > alpha.started; exec sleep 45'".to_string(),
            },
            CoderSpec {
                id: "bravo-coder".to_string(),
                initial_command: "sh -lc 'printf bravo > bravo.started; exec sleep 45'".to_string(),
                resume_command: "sh -lc 'printf bravo > bravo.started; exec sleep 45'".to_string(),
            },
        ],
        &[
            SessionSpec {
                id: "alpha".to_string(),
                name: "alpha".to_string(),
                directory: alpha_directory.clone(),
                coder: "alpha-coder".to_string(),
            },
            SessionSpec {
                id: "bravo".to_string(),
                name: "bravo".to_string(),
                directory: bravo_directory.clone(),
                coder: "bravo-coder".to_string(),
            },
        ],
    );

    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    let report = reconcile_bundle(&config_root, bundle_name, &paths.tmux_socket)
        .expect("bundle reconciliation");
    assert_eq!(report.bootstrap_session.as_deref(), Some("alpha"));
    assert_eq!(report.pruned_sessions, Vec::<String>::new());
    assert_eq!(report.created_sessions.len(), 2);
    assert!(report.created_sessions.contains(&"alpha".to_string()));
    assert!(report.created_sessions.contains(&"bravo".to_string()));

    let owned = list_owned_sessions(&paths.tmux_socket);
    assert_eq!(owned.len(), 2);
    assert!(owned.contains(&"alpha".to_string()));
    assert!(owned.contains(&"bravo".to_string()));

    wait_for_file(
        &alpha_directory.join("alpha.started"),
        Duration::from_millis(800),
    );
    wait_for_file(
        &bravo_directory.join("bravo.started"),
        Duration::from_millis(800),
    );
}

#[test]
fn reconciliation_prunes_stale_owned_sessions_without_killing_non_owned_sessions() {
    if !tmux_available() {
        eprintln!("skipping reconciliation test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(
        temporary.path(),
        bundle_name,
        &[CoderSpec {
            id: "default".to_string(),
            initial_command: "sh -lc 'exec sleep 45'".to_string(),
            resume_command: "sh -lc 'exec sleep 45'".to_string(),
        }],
        &[SessionSpec {
            id: "alpha".to_string(),
            name: "alpha".to_string(),
            directory: temporary.path().to_path_buf(),
            coder: "default".to_string(),
        }],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard::new(paths.tmux_socket.clone());

    let alpha_new = tmux_command(
        &paths.tmux_socket,
        &["new-session", "-d", "-s", "alpha", "sh -lc 'exec sleep 45'"],
    );
    assert!(
        alpha_new.status.success(),
        "failed to create alpha session: {}",
        String::from_utf8_lossy(&alpha_new.stderr)
    );
    let stale_new = tmux_command(
        &paths.tmux_socket,
        &["new-session", "-d", "-s", "stale", "sh -lc 'exec sleep 45'"],
    );
    assert!(
        stale_new.status.success(),
        "failed to create stale session: {}",
        String::from_utf8_lossy(&stale_new.stderr)
    );
    let mark_stale = tmux_command(
        &paths.tmux_socket,
        &["set-option", "-t", "stale", "@agentmux_owned", "1"],
    );
    assert!(
        mark_stale.status.success(),
        "failed to mark stale session owned: {}",
        String::from_utf8_lossy(&mark_stale.stderr)
    );

    let report = reconcile_bundle(&config_root, bundle_name, &paths.tmux_socket)
        .expect("bundle reconciliation");
    assert_eq!(report.created_sessions, Vec::<String>::new());
    assert_eq!(report.pruned_sessions, vec!["stale".to_string()]);

    let list = tmux_command(
        &paths.tmux_socket,
        &["list-sessions", "-F", "#{session_name}"],
    );
    assert!(
        list.status.success(),
        "tmux server should remain when non-owned sessions still exist"
    );
    let sessions = String::from_utf8_lossy(&list.stdout);
    assert!(sessions.lines().any(|line| line.trim() == "alpha"));
}
