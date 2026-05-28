use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use agentmux::{
    relay::{
        ListedBundleState, RelayRequest, RelayResponse, handle_request, reconcile_bundle,
        shutdown_bundle_runtime,
    },
    runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use tempfile::TempDir;

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

struct TmuxServerGuard {
    socket: PathBuf,
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

fn write_tmux_bundle(root: &Path, bundle_name: &str) -> PathBuf {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders");
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
look = "self"
send = "all:home"
"#,
    )
    .expect("write policies");
    let directory = root.display().to_string();
    let body = format!(
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "{directory}"
coder = "shell"
"#
    );
    fs::write(bundles.join(format!("{bundle_name}.toml")), body).expect("write bundle config");
    config_root
}

fn list_bundle(config_root: &Path, bundle_name: &str, runtime_directory: &Path) -> RelayResponse {
    handle_request(
        RelayRequest::List {
            sender_session: Some("alpha".to_string()),
        },
        config_root,
        bundle_name,
        runtime_directory,
    )
    .expect("list response")
}

#[test]
fn list_reports_hosted_round_trip_for_tmux_bundle() {
    if !tmux_available() {
        eprintln!("skipping hosted round-trip test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_tmux_bundle(temporary.path(), bundle_name);
    let paths = BundleRuntimePaths::resolve(temporary.path(), bundle_name).expect("resolve paths");
    ensure_bundle_runtime_directory(&paths).expect("create runtime directory");
    let _tmux_guard = TmuxServerGuard {
        socket: paths.tmux_socket.clone(),
    };

    let pre_response = list_bundle(&config_root, bundle_name, &paths.runtime_directory);
    let RelayResponse::List {
        bundle: pre_bundle, ..
    } = pre_response
    else {
        panic!("expected list response before hosting");
    };
    assert!(
        !pre_bundle.hosted,
        "bundle should not be hosted before reconcile_bundle creates owned sessions"
    );
    assert_eq!(pre_bundle.state, ListedBundleState::Down);

    reconcile_bundle(&config_root, bundle_name, &paths.tmux_socket)
        .expect("reconcile bundle hosting");

    let hosted_response = list_bundle(&config_root, bundle_name, &paths.runtime_directory);
    let RelayResponse::List {
        bundle: hosted_bundle,
        ..
    } = hosted_response
    else {
        panic!("expected list response after hosting");
    };
    assert!(
        hosted_bundle.hosted,
        "bundle should report hosted=true after reconcile created owned tmux session"
    );

    shutdown_bundle_runtime(&paths.tmux_socket).expect("shutdown bundle runtime");

    let post_response = list_bundle(&config_root, bundle_name, &paths.runtime_directory);
    let RelayResponse::List {
        bundle: post_bundle,
        ..
    } = post_response
    else {
        panic!("expected list response after shutdown");
    };
    assert!(
        !post_bundle.hosted,
        "bundle should report hosted=false after shutdown_bundle_runtime pruned owned sessions"
    );
    assert_eq!(post_bundle.state, ListedBundleState::Down);
}
