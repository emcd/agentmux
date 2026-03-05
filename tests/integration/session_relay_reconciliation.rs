use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::json;
use tempfile::TempDir;
use tmuxmux::{
    relay::reconcile_bundle,
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};

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

fn write_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    members: &[serde_json::Value],
) -> PathBuf {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    let body = json!({
        "schema_version": "1",
        "members": members,
    });
    fs::write(
        bundles.join(format!("{bundle_name}.json")),
        serde_json::to_string_pretty(&body).expect("encode bundle"),
    )
    .expect("write bundle config");
    config_root
}

fn list_owned_sessions(socket: &Path) -> Vec<String> {
    let output = tmux_command(
        socket,
        &["list-sessions", "-F", "#{session_name}\t#{@tmuxmux_owned}"],
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
            json!({
                "session_name": "alpha",
                "working_directory": alpha_directory,
                "start_command": "sh -lc 'printf alpha > alpha.started; exec sleep 45'",
            }),
            json!({
                "session_name": "bravo",
                "working_directory": bravo_directory,
                "start_command": "sh -lc 'printf bravo > bravo.started; exec sleep 45'",
            }),
        ],
    );

    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

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

    assert!(alpha_directory.join("alpha.started").exists());
    assert!(bravo_directory.join("bravo.started").exists());

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
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
        &[json!({"session_name": "alpha"})],
    );
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");

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
        &["set-option", "-t", "stale", "@tmuxmux_owned", "1"],
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

    let _ = tmux_command(&paths.tmux_socket, &["kill-server"]);
}
