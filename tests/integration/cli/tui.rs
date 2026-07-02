use std::{fs, process::Command};

use tempfile::TempDir;

use super::helpers::{write_bundle_configuration, write_tui_configuration};

// The interactive TUI no longer rejects a missing default bundle at startup
// (issues/tui/11, issues/runtime/3): a fresh install ships none and the operator
// picks a bundle in the picker. The lenient launch resolution is covered by
// tests/unit/tui_session.rs (resolve_tui_launch_identity); a full headless launch
// cannot be asserted here because terminal initialization requires a TTY.

#[test]
fn tui_rejects_default_session_with_unknown_policy() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "missing", Some("Operator"))],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "tui",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux tui");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_policy"),
        "unexpected stderr: {stderr}"
    );
}
