use agentmux::runtime::paths::{
    BundleRuntimePaths, RelayRuntimePaths, RuntimeRootOverrides, RuntimeRoots,
    agentmux_source_checkout_root, debug_repository_configuration_root,
    debug_repository_inscriptions_root, debug_repository_state_root,
    ensure_bundle_runtime_directory, tmux_socket_path_for_runtime_directory,
};
use tempfile::TempDir;

/// Writes a minimal candidate checkout: optional `.git` entry (directory or
/// marker file, mirroring a primary clone vs a linked worktree) and optional
/// `Cargo.toml` with the given package name.
fn write_checkout_candidate(
    temporary: &TempDir,
    git_entry: Option<GitEntry>,
    package_name: Option<&str>,
) -> std::path::PathBuf {
    let root = temporary.path().join("candidate");
    std::fs::create_dir_all(&root).expect("create candidate root");
    match git_entry {
        Some(GitEntry::Directory) => {
            std::fs::create_dir(root.join(".git")).expect("create .git directory");
        }
        Some(GitEntry::WorktreeFile) => {
            std::fs::write(root.join(".git"), "gitdir: /elsewhere/.git/worktrees/wt\n")
                .expect("write .git worktree file");
        }
        None => {}
    }
    if let Some(name) = package_name {
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\n"),
        )
        .expect("write Cargo.toml");
    }
    root
}

enum GitEntry {
    Directory,
    WorktreeFile,
}

// Debug builds take the dev-mode branch, so the probe's positive and negative
// signals are observable directly. The positive-path tests are ignored in
// release mode because the probe unconditionally returns None without
// debug_assertions.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "probe returns None in release builds")]
fn source_checkout_probe_accepts_agentmux_clone() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = write_checkout_candidate(&temporary, Some(GitEntry::Directory), Some("agentmux"));
    assert_eq!(agentmux_source_checkout_root(&root), Some(root.clone()));
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "probe returns None in release builds")]
fn source_checkout_probe_accepts_linked_worktree() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = write_checkout_candidate(&temporary, Some(GitEntry::WorktreeFile), Some("agentmux"));
    assert_eq!(agentmux_source_checkout_root(&root), Some(root.clone()));
}

#[test]
fn source_checkout_probe_rejects_foreign_git_clone() {
    let temporary = TempDir::new().expect("temporary directory");
    let root =
        write_checkout_candidate(&temporary, Some(GitEntry::Directory), Some("otherproject"));
    assert_eq!(agentmux_source_checkout_root(&root), None);
}

#[test]
fn source_checkout_probe_rejects_git_clone_without_manifest() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = write_checkout_candidate(&temporary, Some(GitEntry::Directory), None);
    assert_eq!(agentmux_source_checkout_root(&root), None);
}

#[test]
fn source_checkout_probe_rejects_source_export_without_git() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = write_checkout_candidate(&temporary, None, Some("agentmux"));
    assert_eq!(agentmux_source_checkout_root(&root), None);
}

#[test]
fn resolves_debug_repository_state_root() {
    let root = debug_repository_state_root(std::path::Path::new("/repo"));
    assert_eq!(
        root,
        std::path::Path::new("/repo/.auxiliary/state/agentmux")
    );
}

#[test]
fn resolves_debug_repository_configuration_root() {
    let root = debug_repository_configuration_root(std::path::Path::new("/repo"));
    assert_eq!(
        root,
        std::path::Path::new("/repo/.auxiliary/configuration/agentmux")
    );
}

#[test]
fn resolves_debug_repository_inscriptions_root() {
    let root = debug_repository_inscriptions_root(std::path::Path::new("/repo"));
    assert_eq!(
        root,
        std::path::Path::new("/repo/.auxiliary/inscriptions/agentmux")
    );
}

#[test]
fn resolves_bundle_runtime_paths() {
    let resolved = BundleRuntimePaths::resolve(std::path::Path::new("/state/root"), "party-alpha")
        .expect("bundle should resolve");
    assert_eq!(
        resolved.runtime_directory,
        std::path::Path::new("/state/root/bundles/party-alpha")
    );
    assert_eq!(
        resolved.tmux_socket,
        std::path::Path::new("/state/root/bundles/party-alpha/tmux.sock")
    );
}

#[test]
fn resolves_relay_runtime_paths_at_state_root() {
    let resolved = RelayRuntimePaths::resolve(std::path::Path::new("/state/root"));
    assert_eq!(
        resolved.relay_socket,
        std::path::Path::new("/state/root/relay.sock")
    );
    assert_eq!(
        resolved.relay_lock_file,
        std::path::Path::new("/state/root/relay.lock")
    );
    assert_eq!(
        resolved.relay_spawn_lock_file,
        std::path::Path::new("/state/root/relay.spawn.lock")
    );
    assert_eq!(
        resolved.relay_ready_sentinel,
        std::path::Path::new("/state/root/relay.ready")
    );
}

#[test]
fn rejects_invalid_bundle_name() {
    let err = BundleRuntimePaths::resolve(std::path::Path::new("/state/root"), "../party")
        .expect_err("bundle should fail");
    assert!(
        err.to_string().contains("invalid bundle name"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolves_roots_from_explicit_overrides() {
    let overrides = RuntimeRootOverrides {
        configuration_root: Some("/configuration".into()),
        state_root: Some("/state".into()),
        inscriptions_root: Some("/inscriptions".into()),
        repository_root: None,
    };
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/configuration")
    );
    assert_eq!(roots.state_root, std::path::Path::new("/state"));
    assert_eq!(
        roots.inscriptions_root,
        std::path::Path::new("/inscriptions")
    );
}

#[test]
fn creates_bundle_runtime_directory() {
    let temporary = TempDir::new().expect("temporary");
    let paths = BundleRuntimePaths::resolve(temporary.path(), "party-alpha").expect("paths");
    ensure_bundle_runtime_directory(&paths).expect("directory");
    assert!(paths.runtime_directory.is_dir());
}

#[test]
fn derives_tmux_socket_from_runtime_directory() {
    let runtime_directory = std::path::Path::new("/state/root/bundles/party-alpha");
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    assert_eq!(
        tmux_socket,
        std::path::Path::new("/state/root/bundles/party-alpha/tmux.sock")
    );
}
