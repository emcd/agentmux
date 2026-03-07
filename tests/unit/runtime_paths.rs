use tempfile::TempDir;
use tmuxmux::runtime::paths::{
    BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots, debug_repository_configuration_root,
    debug_repository_state_root, ensure_bundle_runtime_directory,
};

#[test]
fn resolves_debug_repository_state_root() {
    let root = debug_repository_state_root(std::path::Path::new("/repo"));
    assert_eq!(root, std::path::Path::new("/repo/.auxiliary/state/tmuxmux"));
}

#[test]
fn resolves_debug_repository_configuration_root() {
    let root = debug_repository_configuration_root(std::path::Path::new("/repo"));
    assert_eq!(
        root,
        std::path::Path::new("/repo/.auxiliary/configuration/tmuxmux")
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
    assert_eq!(
        resolved.relay_socket,
        std::path::Path::new("/state/root/bundles/party-alpha/relay.sock")
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
        repository_root: None,
    };
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/configuration")
    );
    assert_eq!(roots.state_root, std::path::Path::new("/state"));
}

#[test]
fn creates_bundle_runtime_directory() {
    let temporary = TempDir::new().expect("temporary");
    let paths = BundleRuntimePaths::resolve(temporary.path(), "party-alpha").expect("paths");
    ensure_bundle_runtime_directory(&paths).expect("directory");
    assert!(paths.runtime_directory.is_dir());
}
