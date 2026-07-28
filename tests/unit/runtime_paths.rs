use std::{
    path::{Path, PathBuf},
    process::Command,
};

use agentmux::runtime::paths::{
    BundleRuntimePaths, ConfigurationRootSource, RelayRuntimePaths, RuntimeRootOverrides,
    RuntimeRoots, debug_repository_inscriptions_root, debug_repository_state_root,
    ensure_bundle_runtime_directory, repository_checkout_root,
    tmux_socket_path_for_runtime_directory,
};
use tempfile::TempDir;

fn run_git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .args(arguments)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git command failed: git {}\nstdout:\n{}\nstderr:\n{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Initializes a real repository with one commit, so `git worktree add` has
/// something to link against. The resolver reads Git's own answer rather than a
/// fabricated layout, which is the only way the worktree case is exercised
/// honestly.
fn initialize_repository(root: &Path) {
    std::fs::create_dir_all(root).expect("create repository root");
    run_git(root, &["init"]);
    run_git(
        root,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test User",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
}

/// Writes the package manifest marker distinguishing an Agentmux checkout from
/// any other repository the process might be standing in.
fn write_manifest(root: &Path, package_name: &str) {
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{package_name}\"\nversion = \"0.0.0\"\n"),
    )
    .expect("write Cargo.toml");
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().expect("canonicalize path")
}

// Repository-local roots are reachable only under debug_assertions, where the
// resolver does its work; in release it short-circuits to None by design, so the
// positive cases have nothing to assert.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "repository-local roots are unreachable in release builds"
)]
fn repository_checkout_root_resolves_an_agentmux_checkout() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("agentmux");
    initialize_repository(&root);
    write_manifest(&root, "agentmux");

    let checkout = repository_checkout_root(&root).expect("checkout must resolve");
    assert_eq!(resolved(&checkout), resolved(&root));
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "repository-local roots are unreachable in release builds"
)]
fn repository_checkout_root_resolves_from_a_nested_working_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("agentmux");
    initialize_repository(&root);
    write_manifest(&root, "agentmux");
    let nested = root.join("src/relay");
    std::fs::create_dir_all(&nested).expect("create nested directory");

    // Git searches ancestors, so a process launched anywhere beneath a checkout
    // resolves the same root as one launched at its top.
    let checkout = repository_checkout_root(&nested).expect("checkout must resolve");
    assert_eq!(resolved(&checkout), resolved(&root));
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "repository-local roots are unreachable in release builds"
)]
fn linked_worktree_resolves_the_common_dir_owner_repository_root() {
    let temporary = TempDir::new().expect("temporary directory");
    let project_root = temporary.path().join("agentmux");
    initialize_repository(&project_root);
    write_manifest(&project_root, "agentmux");

    let worktree_root = temporary.path().join("WORKTREES/agentmux/relay");
    std::fs::create_dir_all(worktree_root.parent().expect("worktree parent"))
        .expect("create worktree parent");
    run_git(
        &project_root,
        &[
            "worktree",
            "add",
            "--detach",
            worktree_root.to_str().expect("utf8 path"),
        ],
    );

    // The owner root, not the worktree. Every worktree of a checkout must answer
    // with one path, or siblings bind private relay sockets and cannot reach
    // each other.
    let checkout = repository_checkout_root(&worktree_root).expect("worktree must resolve a root");
    assert_eq!(resolved(&checkout), resolved(&project_root));
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "repository-local roots are unreachable in release builds"
)]
fn repository_local_state_and_inscriptions_activate_for_a_source_checkout() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("agentmux");
    initialize_repository(&root);
    write_manifest(&root, "agentmux");

    // The end the resolver serves: an unresolved root would silently collapse
    // repository-local state onto the XDG default, which is the coexistence
    // failure the retained provenance exists to prevent.
    let checkout = repository_checkout_root(&root).expect("checkout must resolve");
    let roots = RuntimeRoots::resolve(&RuntimeRootOverrides {
        configuration_root: Some(temporary.path().join("configuration")),
        repository_root: Some(checkout.clone()),
        ..RuntimeRootOverrides::default()
    })
    .expect("resolve runtime roots");

    assert_eq!(roots.state_root, debug_repository_state_root(&checkout));
    assert_eq!(
        roots.inscriptions_root,
        debug_repository_inscriptions_root(&checkout)
    );
}

#[test]
fn repository_checkout_root_rejects_a_foreign_repository() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("otherproject");
    initialize_repository(&root);
    write_manifest(&root, "otherproject");

    assert_eq!(repository_checkout_root(&root), None);
}

#[test]
fn repository_checkout_root_rejects_a_repository_without_a_manifest() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("unmarked");
    initialize_repository(&root);

    assert_eq!(repository_checkout_root(&root), None);
}

#[test]
fn repository_checkout_root_rejects_a_source_export_without_git() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("export");
    std::fs::create_dir_all(&root).expect("create export root");
    write_manifest(&root, "agentmux");

    assert_eq!(repository_checkout_root(&root), None);
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

/// Builds overrides naming no configuration root, so resolution falls to the
/// tiers below the command line. State and inscriptions are pinned so a test
/// observing the configuration tier is not also exercising theirs.
fn unnamed_configuration_overrides() -> RuntimeRootOverrides {
    RuntimeRootOverrides {
        configuration_root: None,
        state_root: Some("/state".into()),
        inscriptions_root: Some("/inscriptions".into()),
        repository_root: None,
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

#[test]
fn configuration_root_resolves_from_environment_when_no_flag() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/from-environment");
    }

    let roots =
        RuntimeRoots::resolve(&unnamed_configuration_overrides()).expect("roots should resolve");
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

    let mut overrides = unnamed_configuration_overrides();
    overrides.configuration_root = Some("/from-flag".into());
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(roots.configuration_root, std::path::Path::new("/from-flag"));
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::CommandLine
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "repository-local roots are unreachable in release builds"
)]
fn repository_root_no_longer_selects_the_configuration_root() {
    // It retains its state and inscriptions role; only the configuration-root
    // role is gone. The state-root assertion below depends on the
    // repository-local branch of resolve_state_root, which is reachable only
    // under debug_assertions (see src/runtime/paths.rs), so this test is
    // gated like its siblings above rather than asserted profile-invariantly.
    clear_configuration_environment();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }
    let temporary = TempDir::new().expect("temporary");
    let repository_root = temporary.path().join("repo");
    // Planted so a reintroduced repository-local configuration branch would
    // have something to find; the path is spelled out because no resolver
    // derives it any more.
    std::fs::create_dir_all(repository_root.join(".auxiliary/configuration/agentmux"))
        .expect("create repository configuration root");

    let overrides = RuntimeRootOverrides {
        configuration_root: None,
        state_root: None,
        inscriptions_root: None,
        repository_root: Some(repository_root.clone()),
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
