use std::path::PathBuf;

use agentmux::runtime::paths::{
    BundleRuntimePaths, ConfigurationRootSource, RelayRuntimePaths, RuntimeRootOverrides,
    RuntimeRoots, ensure_bundle_runtime_directory, tmux_socket_path_for_runtime_directory,
};
use tempfile::TempDir;

/// Clears every environment variable feeding state-root resolution, so a test
/// observes the tier it is exercising rather than the developer's shell.
///
/// Safe because nextest runs each test in its own process.
fn clear_state_environment() {
    unsafe {
        std::env::remove_var("AGENTMUX_STATE_DIRECTORY");
        std::env::remove_var("XDG_STATE_HOME");
    }
}

/// Builds overrides naming no state root, so resolution falls to the tiers
/// below the command line. Configuration is pinned so a test observing the
/// state tier is not also exercising its resolution.
fn unnamed_state_overrides() -> RuntimeRootOverrides {
    RuntimeRootOverrides {
        configuration_layers: vec!["/configuration".into()],
        state_root: None,
        inscriptions_root: None,
    }
}

#[test]
fn environment_tier_selects_the_state_root() {
    clear_state_environment();
    unsafe {
        std::env::set_var("AGENTMUX_STATE_DIRECTORY", "/from-environment");
        std::env::set_var("XDG_STATE_HOME", "/xdg");
    }

    let roots = RuntimeRoots::resolve(&unnamed_state_overrides()).expect("roots should resolve");

    assert_eq!(roots.state_root, PathBuf::from("/from-environment"));
    assert_eq!(
        roots.inscriptions_root,
        PathBuf::from("/from-environment/inscriptions"),
        "inscriptions follow the state root rather than being selected separately"
    );
}

#[test]
fn explicit_state_directory_outranks_the_environment_tier() {
    clear_state_environment();
    unsafe {
        std::env::set_var("AGENTMUX_STATE_DIRECTORY", "/from-environment");
    }

    let mut overrides = unnamed_state_overrides();
    overrides.state_root = Some("/from-flag".into());
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");

    assert_eq!(roots.state_root, PathBuf::from("/from-flag"));
}

#[test]
fn a_blank_state_environment_value_is_absent_rather_than_empty() {
    // Blank must mean "this tier said nothing", not "the state root is the
    // empty path". Reading it as a value would resolve every state artifact
    // against the working directory.
    clear_state_environment();
    unsafe {
        std::env::set_var("AGENTMUX_STATE_DIRECTORY", "   ");
        std::env::set_var("XDG_STATE_HOME", "/xdg");
    }

    let roots = RuntimeRoots::resolve(&unnamed_state_overrides()).expect("roots should resolve");

    assert_eq!(roots.state_root, PathBuf::from("/xdg/agentmux"));
}

#[test]
fn an_empty_state_directory_flag_is_rejected() {
    clear_state_environment();
    let mut overrides = unnamed_state_overrides();
    overrides.state_root = Some(PathBuf::new());

    let error = RuntimeRoots::resolve(&overrides)
        .expect_err("an empty state directory must fault rather than resolve");
    assert!(
        error
            .to_string()
            .contains("validation_invalid_state_directory"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_relative_state_root_normalizes_against_the_working_directory() {
    // Propagation depends on this. A relative root re-resolves under each
    // spawned child's working directory, so the stamped value would name a
    // different directory for every member that declares its own.
    clear_state_environment();
    let mut overrides = unnamed_state_overrides();
    overrides.state_root = Some("relative-state".into());

    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");

    assert!(
        roots.state_root.is_absolute(),
        "state root must be absolute, got {}",
        roots.state_root.display()
    );
    let working_directory = std::env::current_dir().expect("working directory");
    assert_eq!(roots.state_root, working_directory.join("relative-state"));
    assert_eq!(
        roots.inscriptions_root,
        working_directory.join("relative-state/inscriptions"),
        "the absolute root is what downstream resolution uses"
    );
}

#[test]
fn a_relative_configuration_layer_normalizes_against_the_working_directory() {
    // Same precondition as the state root, and for the same reason: the layer
    // list is stamped into every coder-backed member's environment, so a
    // relative layer would name a different directory for each member that
    // declares its own working directory, while both appear to name the layer
    // the relay resolved.
    clear_configuration_environment();
    let mut overrides = unnamed_configuration_overrides();
    overrides.configuration_layers = vec!["relative-config".into(), "also-relative".into()];

    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");

    let working_directory = std::env::current_dir().expect("working directory");
    let layers = roots.configuration_roots.layers();
    assert_eq!(
        layers,
        [
            working_directory.join("relative-config"),
            working_directory.join("also-relative")
        ],
        "every layer is absolutized, and list order is preserved"
    );
}

#[test]
fn absolutizing_a_configuration_layer_does_not_resolve_symlinks() {
    // Lexical absolutization, not canonicalization. Symlinked configuration
    // paths are ordinary here, and rewriting an operator's declared layer into
    // its target names a path they did not choose -- which then reaches every
    // member through the stamp.
    clear_configuration_environment();
    let temporary = TempDir::new().expect("temporary");
    let target = temporary.path().join("actual-config");
    std::fs::create_dir_all(&target).expect("create target");
    let link = temporary.path().join("linked-config");
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");

    let mut overrides = unnamed_configuration_overrides();
    overrides.configuration_layers = vec![link.clone()];

    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");

    assert_eq!(
        roots.configuration_roots.layers(),
        [link],
        "the declared path survives; only relativity is removed"
    );
}

#[test]
fn build_profile_does_not_change_the_resolved_roots() {
    // The assertion the deleted `cfg!(debug_assertions)` branches made
    // impossible. Written profile-invariantly on purpose: the same arguments
    // must produce the same roots whichever profile compiled this test.
    clear_state_environment();
    unsafe {
        std::env::set_var("XDG_STATE_HOME", "/xdg");
    }

    let roots = RuntimeRoots::resolve(&unnamed_state_overrides()).expect("roots should resolve");

    assert_eq!(roots.state_root, PathBuf::from("/xdg/agentmux"));
    assert_eq!(
        roots.inscriptions_root,
        PathBuf::from("/xdg/agentmux/inscriptions")
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
        configuration_layers: vec!["/configuration".into()],
        state_root: Some("/state".into()),
        inscriptions_root: Some("/inscriptions".into()),
    };
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(
        roots.configuration_roots.layers(),
        [PathBuf::from("/configuration")]
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
        configuration_layers: Vec::new(),
        state_root: Some("/state".into()),
        inscriptions_root: Some("/inscriptions".into()),
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
        roots.configuration_roots.layers(),
        [PathBuf::from("/from-environment")]
    );
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::Environment
    );
}

#[test]
fn a_supplied_layer_list_never_reaches_the_xdg_default() {
    // Closedness. A supplied list replaces the tier stack rather than extending
    // it, so a typo names a root that does not exist rather than silently
    // resolving the operator's real configuration from the tier below — the
    // demotion that naming a root exists to prevent.
    clear_configuration_environment();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }

    let mut overrides = unnamed_configuration_overrides();
    overrides.configuration_layers = vec!["/typo'd".into()];
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");

    assert_eq!(
        roots.configuration_roots.layers(),
        [PathBuf::from("/typo'd")],
        "a supplied list must not be extended by the default tier"
    );
}

#[test]
fn a_supplied_environment_list_never_reaches_the_xdg_default() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/rnd:/base");
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
    }

    let roots =
        RuntimeRoots::resolve(&unnamed_configuration_overrides()).expect("roots should resolve");

    assert_eq!(
        roots.configuration_roots.layers(),
        [PathBuf::from("/rnd"), PathBuf::from("/base")],
        "a supplied list must not be extended by the default tier"
    );
}

#[test]
fn an_empty_layer_element_is_rejected_at_resolution() {
    // The classic search-path trap: an empty element read as the working
    // directory would admit configuration from wherever a process was started.
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/rnd::/base");
    }

    let error = RuntimeRoots::resolve(&unnamed_configuration_overrides())
        .expect_err("an empty element must fault rather than contribute a layer");
    assert!(
        error
            .to_string()
            .contains("validation_invalid_configuration_layers"),
        "unexpected error: {error}"
    );
}

#[test]
fn explicit_flag_outranks_environment() {
    clear_configuration_environment();
    unsafe {
        std::env::set_var("AGENTMUX_CONFIGURATION_DIRECTORY", "/from-environment");
    }

    let mut overrides = unnamed_configuration_overrides();
    overrides.configuration_layers = vec!["/from-flag".into()];
    let roots = RuntimeRoots::resolve(&overrides).expect("roots should resolve");
    assert_eq!(
        roots.configuration_roots.layers(),
        [PathBuf::from("/from-flag")]
    );
    assert_eq!(
        roots.configuration_root_source,
        ConfigurationRootSource::CommandLine
    );
}
