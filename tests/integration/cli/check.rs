//! CLI coverage for `agentmux check configuration` — the read-only
//! configuration pre-flight subcommand.

use std::{fs, process::Command};

use tempfile::TempDir;

use super::helpers::*;

fn config_and_state(temporary: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    (config_root, state_root)
}

#[test]
fn check_configuration_rejects_a_supplied_layer_that_does_not_exist() {
    // The failure this guards is silent, not loud: the base layer is entirely
    // valid, so without a layer-existence check the command validates it, exits
    // zero, and reports the deployment healthy while every override the operator
    // wrote is inert. Pre-flight is the command run to catch exactly that.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let absent_override = temporary.path().join("override-typo");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            absent_override.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        !output.status.success(),
        "a missing supplied layer must fault rather than resolve from the layers below it; \
         stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_configuration_root_absent"),
        "expected validation_configuration_root_absent, got: {stderr}"
    );
    assert!(
        stderr.contains("override-typo"),
        "the reported path must name the layer at fault: {stderr}"
    );
}

#[test]
fn check_configuration_accepts_a_layer_list_whose_layers_all_exist() {
    // The companion to the test above: the existence check must not reject a
    // legitimate multi-layer list, including a layer that contributes no bundles.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(&override_layer).expect("create override layer");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            override_layer.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "an existing empty layer must not fault: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok: alpha"), "unexpected stdout: {stdout}");
}

#[test]
fn check_configuration_accepts_valid_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "alpha",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok: alpha"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("all valid"), "unexpected stdout: {stdout}");
}

#[test]
fn check_configuration_validates_all_bundles_when_no_id() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    write_bundle_configuration(&config_root, "bravo", None, &["b"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok: alpha"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("ok: bravo"), "unexpected stdout: {stdout}");
}

#[test]
fn check_configuration_reports_unknown_field_with_detail() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    // Overwrite the bundle with the headline incident: a misspelled session key
    // (`codex-session-id` instead of `coder`). `deny_unknown_fields` rejects it.
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        r#"format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "/tmp"
coder = "default"
codex-session-id = "a"
"#,
    )
    .expect("overwrite bundle with bad field");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "alpha",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("codex-session-id"),
        "stderr should name the offending field: {stderr}"
    );
    assert!(
        stderr.contains("alpha.toml"),
        "stderr should name the offending file: {stderr}"
    );
}

#[test]
fn check_configuration_reports_no_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no bundle configurations found"),
        "unexpected stderr: {stderr}"
    );
}

// A malformed relay.toml is reported even when the config root has no bundles:
// relay-level validation runs before bundle discovery, matching relay startup
// (which rejects the same artifact up front) rather than short-circuiting on the
// no-bundles error.
#[test]
fn check_configuration_reports_invalid_relay_toml_without_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    fs::write(
        config_root.join("relay.toml"),
        "[[peers]]\nalias = \"west\"\naddress = \"\"\nconnect-as = \"east\"\n",
    )
    .expect("write relay.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peers.address"),
        "stderr should name the offending relay.toml field: {stderr}"
    );
    assert!(
        !stderr.contains("no bundle configurations found"),
        "relay.toml validation must run before the no-bundles check: {stderr}"
    );
}

// A malformed ui.toml is reported at the config-root level, like relay.toml,
// even when no bundles exist: UI-surface config validation runs up front so a
// bad ui.toml fails pre-flight rather than only at TUI/CLI startup.
#[test]
fn check_configuration_reports_invalid_ui_toml_without_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    fs::write(config_root.join("ui.toml"), "unknown-key = \"oops\"\n").expect("write ui.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ui.toml"),
        "stderr should name the offending ui.toml file: {stderr}"
    );
    assert!(
        !stderr.contains("no bundle configurations found"),
        "ui.toml validation must run before the no-bundles check: {stderr}"
    );
}

#[test]
fn check_configuration_accepts_valid_ui_toml() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("ui.toml"), "default-bundle = \"alpha\"\n").expect("write ui.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "valid ui.toml should not fail pre-flight: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_configuration_reports_the_layer_that_supplied_each_artifact() {
    // The shadowing case layering introduces: two valid copies of one bundle,
    // where nothing is wrong and the operator's only question is which copy is in
    // effect. Resolution is per file, not per layer, so this asserts both
    // directions — the shadowed bundle resolves from the override while
    // `coders.toml`, which the override does not supply, resolves from the base.
    // An implementation reporting a single layer for everything fails one of them
    // whichever layer it picked.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(override_layer.join("bundles")).expect("create override bundles directory");
    fs::write(
        override_layer.join("bundles").join("alpha.toml"),
        "format-version = 1\n\n[[sessions]]\nid = \"shadowing\"\nname = \"shadowing\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
    )
    .expect("write shadowing bundle");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            override_layer.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "both copies are valid: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shadowing = override_layer.join("bundles").join("alpha.toml");
    let shadowed = config_root.join("bundles").join("alpha.toml");
    assert!(
        stdout.contains(&format!(
            "source bundles/alpha.toml: {}",
            shadowing.display()
        )),
        "the bundle must be reported against the layer supplying it: {stdout}"
    );
    assert!(
        !stdout.contains(shadowed.to_str().expect("shadowed utf8")),
        "the shadowed copy must not appear as a source: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "source coders.toml: {}",
            config_root.join("coders.toml").display()
        )),
        "an artifact the override does not supply must resolve from the base: {stdout}"
    );
    // `ui.toml` exists in no layer. Reporting a synthesized path for it would
    // claim a file that is not there.
    assert!(
        !stdout.contains("source ui.toml"),
        "an artifact no layer supplies must contribute no line: {stdout}"
    );
}

#[test]
fn check_configuration_reports_the_layer_that_supplied_the_association_file() {
    // `mcp.toml` selects the bundle and session an MCP server binds to, so a
    // shadowed copy silently redirects an association — the most consequential
    // shadowing this report exists to expose, and the one no other surface shows.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("mcp.toml"), "bundle_name = \"alpha\"\n")
        .expect("write base mcp.toml");
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(&override_layer).expect("create override layer");
    fs::write(
        override_layer.join("mcp.toml"),
        "bundle_name = \"alpha\"\nsession_name = \"a\"\n",
    )
    .expect("write override mcp.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            override_layer.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(
        output.status.success(),
        "both copies are valid: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "source mcp.toml: {}",
            override_layer.join("mcp.toml").display()
        )),
        "the association file must be reported against the layer supplying it: {stdout}"
    );
    assert!(
        !stdout.contains(
            config_root
                .join("mcp.toml")
                .to_str()
                .expect("shadowed utf8")
        ),
        "the shadowed association file must not appear as a source: {stdout}"
    );
}

#[test]
fn check_configuration_reports_sources_before_rejecting_a_missing_layer() {
    // The report is worth most on exactly this run: every artifact resolves from
    // the layers beneath the missing one, which is the diagnosis, and the error
    // then names the layer responsible. Reporting after layer validation would
    // blank it here.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let absent_override = temporary.path().join("override-typo");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            absent_override.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "a missing layer must still fault");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "source coders.toml: {}",
            config_root.join("coders.toml").display()
        )),
        "sources must be reported before the layer-existence check: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_configuration_root_absent"),
        "the layer fault must still be reported: {stderr}"
    );
}

#[test]
fn check_configuration_reports_sources_ahead_of_a_validation_failure() {
    // Validation is fail-fast, so a report interleaved with it would stop at the
    // first invalid artifact — the run where the whole picture is most wanted.
    // Both streams are merged into one pipe here because the ordering under test
    // is the one an operator captures, not the one each stream sees alone.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        "format-version = 1\n\n[[sessions]]\nid = \"a\"\nname = \"a\"\ndirectory = \"/tmp\"\ncoder = \"default\"\ncodex-session-id = \"a\"\n",
    )
    .expect("overwrite bundle with bad field");

    let output = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{binary} check configuration alpha --configuration-directory {config} --state-directory {state} 2>&1",
            binary = env!("CARGO_BIN_EXE_agentmux"),
            config = config_root.display(),
            state = state_root.display(),
        ))
        .output()
        .expect("run agentmux check configuration with merged streams");

    assert!(!output.status.success(), "command should fail");
    let merged = String::from_utf8_lossy(&output.stdout);
    let report = merged
        .find("source coders.toml")
        .unwrap_or_else(|| panic!("sources must be reported on a failing run: {merged}"));
    let failure = merged
        .find("codex-session-id")
        .unwrap_or_else(|| panic!("the failure must be reported: {merged}"));
    assert!(
        report < failure,
        "the source report must precede the failure explaining it: {merged}"
    );
}

#[test]
fn check_configuration_quiet_suppresses_success_output() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--quiet",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration --quiet");

    assert!(
        output.status.success(),
        "quiet must not change the verdict: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "quiet must suppress sources, per-bundle lines, and the summary alike: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn check_configuration_quiet_still_reports_a_failure() {
    // Quiet serves the operator who wants the exit code alone; it must not
    // suppress the diagnosis, which is the one output a failing run exists to
    // produce. `-q` is exercised here so both spellings are covered.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(
        config_root.join("bundles").join("alpha.toml"),
        "format-version = 1\n\n[[sessions]]\nid = \"a\"\nname = \"a\"\ndirectory = \"/tmp\"\ncoder = \"default\"\ncodex-session-id = \"a\"\n",
    )
    .expect("overwrite bundle with bad field");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "alpha",
            "-q",
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration -q");

    assert!(!output.status.success(), "command should fail");
    assert!(
        output.stdout.is_empty(),
        "quiet must still suppress success output on a failing run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("codex-session-id"),
        "quiet must not suppress the diagnosis: {stderr}"
    );
}

#[test]
fn check_configuration_keeps_the_layer_code_when_the_fault_arrives_through_a_root_artifact() {
    // `ui.toml` is loaded through the configuration module rather than the relay
    // pre-flight, so it crosses a different error mapper on its way out. Every
    // failure exits `1`, which leaves the structured code as the only thing
    // distinguishing "a layer could not be read" from "a file is malformed" —
    // so a mapper that folds the first into the second erases the distinction
    // at the one command that exists to report it.
    //
    // A directory occupying the artifact path, rather than a permission fixture:
    // it faults for the same reason, and it runs everywhere the suite does. The
    // relay artifacts resolve cleanly, so the run reaches the `ui.toml` mapper
    // instead of failing earlier on a layer-wide fault.
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(override_layer.join("ui.toml")).expect("occupy the ui.toml path");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "check",
            "configuration",
            "--configuration-directory",
            override_layer.to_str().expect("override utf8"),
            "--configuration-directory",
            config_root.to_str().expect("config root utf8"),
            "--state-directory",
            state_root.to_str().expect("state root utf8"),
        ])
        .output()
        .expect("run agentmux check configuration");

    assert!(!output.status.success(), "the run must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unreadable_configuration_layer"),
        "the layer fault must keep its code across the configuration mapper: {stderr}"
    );
    assert!(
        stderr.contains(&override_layer.join("ui.toml").display().to_string()),
        "the failure must name the occupied path: {stderr}"
    );
}

#[test]
fn check_configuration_rejects_unknown_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["check", "everything"])
        .output()
        .expect("run agentmux check everything");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown check subcommand"),
        "unexpected stderr: {stderr}"
    );
}
