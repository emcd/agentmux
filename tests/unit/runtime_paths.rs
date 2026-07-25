use agentmux::runtime::paths::{
    BundleRuntimePaths, ConfigurationRootSource, RelayRuntimePaths, RuntimeRootOverrides,
    RuntimeRoots, agentmux_source_checkout_root, debug_repository_inscriptions_root,
    debug_repository_state_root, ensure_bundle_runtime_directory, local_configuration_root,
    tmux_socket_path_for_runtime_directory,
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
fn resolves_local_configuration_root() {
    let root = local_configuration_root(std::path::Path::new("/repo"));
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
        discover_local_configuration: false,
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
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::CommandLine
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

/// Builds overrides naming no root, so resolution falls to the tiers below the
/// command line.
fn discovery_overrides(discover: bool) -> RuntimeRootOverrides {
    RuntimeRootOverrides {
        configuration_root: None,
        state_root: Some("/state".into()),
        inscriptions_root: Some("/inscriptions".into()),
        repository_root: None,
        discover_local_configuration: discover,
    }
}

/// Clears every environment variable feeding configuration-root resolution, so
/// a test observes the tier it is exercising rather than the developer's shell.
///
/// Safe because nextest runs each test in its own process.
fn clear_configuration_environment() {
    unsafe {
        std::env::remove_var("AGENTMUX_CONFIGURATION_DIRECTORY");
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}

/// Lays out `<root>/repo/nested/deep`, planting a configuration root marker at
/// each requested ancestor. Returns the canonicalized deep directory.
fn write_discovery_tree(temporary: &TempDir, markers: &[&str]) -> std::path::PathBuf {
    let deep = temporary.path().join("repo/nested/deep");
    std::fs::create_dir_all(&deep).expect("create deep directory");
    for marker in markers {
        let ancestor = temporary.path().join(marker);
        std::fs::create_dir_all(local_configuration_root(&ancestor)).expect("create marker");
    }
    deep.canonicalize().expect("canonicalize deep directory")
}

#[test]
fn configuration_root_resolves_from_environment_when_no_flag() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/from-environment");
    }

    let roots = RuntimeRoots::resolve(&discovery_overrides(false)).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/from-environment")
    );
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::Environment
    );
}

#[test]
fn explicit_flag_outranks_environment() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/from-environment");
    }

    let mut overrides = discovery_overrides(false);
    overrides.configuration_root = Some("/from-flag".into());
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(roots.configuration_root, std::path::Path::new("/from-flag"));
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::CommandLine
    );
}

#[test]
fn discovery_is_inert_unless_requested() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }
    let temporary = TempDir::new().expect("temporary");
    let deep = write_discovery_tree(&temporary, &["repo"]);
    std::env::set_current_dir(&deep).expect("enter deep directory");

    let roots = RuntimeRoots::resolve(&discovery_overrides(false)).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/xdg/agentmux")
    );
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::Default
    );
}

#[test]
fn discovery_selects_the_nearest_ancestor() {
    clear_configuration_environment();
    let temporary = TempDir::new().expect("temporary");
    let deep = write_discovery_tree(&temporary, &["repo", "repo/nested"]);
    std::env::set_current_dir(&deep).expect("enter deep directory");

    let roots = RuntimeRoots::resolve(&discovery_overrides(true)).expect("roots should resolve");
    let expected = local_configuration_root(&temporary.path().join("repo/nested"))
        .canonicalize()
        .expect("canonicalize expected root");
    assert_eq!(roots.configuration_root, expected);
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::Discovered
    );
}

#[test]
fn discovery_walks_past_ancestors_without_a_marker() {
    clear_configuration_environment();
    let temporary = TempDir::new().expect("temporary");
    let deep = write_discovery_tree(&temporary, &["repo"]);
    std::env::set_current_dir(&deep).expect("enter deep directory");

    let roots = RuntimeRoots::resolve(&discovery_overrides(true)).expect("roots should resolve");
    let expected = local_configuration_root(&temporary.path().join("repo"))
        .canonicalize()
        .expect("canonicalize expected root");
    assert_eq!(roots.configuration_root, expected);
}

#[test]
fn discovery_falls_through_when_no_ancestor_qualifies() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }
    let temporary = TempDir::new().expect("temporary");
    let deep = write_discovery_tree(&temporary, &[]);
    std::env::set_current_dir(&deep).expect("enter deep directory");

    let roots = RuntimeRoots::resolve(&discovery_overrides(true)).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/xdg/agentmux")
    );
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::Default
    );
}

#[test]
fn repository_root_no_longer_selects_the_configuration_root() {
    // It retains its state and inscriptions role; only the configuration-root
    // role is gone. Asserted without a build-profile guard, because
    // configuration-root resolution no longer varies by profile.
    clear_configuration_environment();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }
    let temporary = TempDir::new().expect("temporary");
    let repository_root = temporary.path().join("repo");
    std::fs::create_dir_all(local_configuration_root(&repository_root))
        .expect("create repository configuration root");

    let overrides = RuntimeRootOverrides {
        configuration_root: None,
        state_root: None,
        inscriptions_root: None,
        repository_root: Some(repository_root.clone()),
        discover_local_configuration: false,
    };
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(
        roots.configuration_root,
        std::path::Path::new("/xdg/agentmux"),
        "repository root must not supply the configuration root"
    );
    assert_eq!(
        roots.state_root,
        debug_repository_state_root(&repository_root)
    );
}
