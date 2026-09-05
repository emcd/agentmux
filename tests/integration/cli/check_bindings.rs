//! CLI coverage for the binding group in `agentmux check configuration`.
//!
//! Filed apart from `check.rs`, which asks whether pre-flight validates the
//! configuration artifacts at all. These ask the narrower question the binding
//! work added: what pre-flight says about a binding group that loads, and which
//! of its outcomes is a report rather than a refusal.

use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

use super::helpers::*;

fn config_and_state(temporary: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    (config_root, state_root)
}

/// Runs pre-flight over the given layers, front to back.
fn check(layers: &[&Path], state_root: &Path) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
    command.args(["check", "configuration"]);
    for layer in layers {
        command.args([
            "--configuration-directory",
            layer.to_str().expect("layer utf8"),
        ]);
    }
    command.args([
        "--state-directory",
        state_root.to_str().expect("state utf8"),
    ]);
    command.output().expect("run agentmux check configuration")
}

/// A group taking sending off the compose field.
///
/// All three chords, because each of compose's sending rows is written as one
/// exact keystroke and claiming one leaves the other two answering. A behavior
/// sitting on a control chord instead cannot be displaced this way at all: that
/// shape matches every modifier set containing `Ctrl`, so a single row leaves
/// seven combinations still reaching it.
const DISPLACES_SENDING: &str = "[bindings.compose-message]\n\
     \"enter\" = \"insert-message-newline\"\n\
     \"shift+enter\" = \"insert-message-newline\"\n\
     \"ctrl+enter\" = \"insert-message-newline\"\n";

/// The same, stated for one terminal class only.
const DISPLACES_SENDING_ON_ENHANCED: &str = "[bindings.compose-message]\n\
     \"enter\" = { enhanced = \"insert-message-newline\" }\n\
     \"shift+enter\" = { enhanced = \"insert-message-newline\" }\n\
     \"ctrl+enter\" = { enhanced = \"insert-message-newline\" }\n";

/// A displacement is reported and the run still succeeds. An operator may
/// intend it, so pre-flight describes the outcome rather than judging it.
#[test]
fn check_configuration_reports_a_displaced_behavior_without_failing() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("ui.toml"), DISPLACES_SENDING).expect("write ui.toml");

    let output = check(&[&config_root], &state_root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a displacement is a report, not a refusal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(
            "binding finding: send-message (Message: send) is unreachable \
             in compose-message under both terminal classes"
        ),
        "pre-flight did not report the displaced behavior: {stdout}"
    );
}

/// A class-qualified row leaves the other class alone, and the report says
/// which class it holds under rather than reporting it twice.
#[test]
fn check_configuration_names_the_single_class_a_finding_holds_under() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("ui.toml"), DISPLACES_SENDING_ON_ENHANCED).expect("write ui.toml");

    let output = check(&[&config_root], &state_root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("is unreachable in compose-message under the enhanced terminal class"),
        "pre-flight did not name the single class: {stdout}"
    );
    assert_eq!(
        stdout.matches("send-message").count(),
        1,
        "a finding holding under one class was reported more than once: {stdout}"
    );
}

/// The control the two above rest on: an unconfigured run reports no finding at
/// all. Without it, a report that named every behavior would satisfy them.
#[test]
fn check_configuration_reports_no_binding_finding_for_the_shipped_defaults() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("ui.toml"), "default-bundle = \"alpha\"\n").expect("write ui.toml");

    let output = check(&[&config_root], &state_root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        !stdout.contains("binding finding:"),
        "the shipped defaults leave nothing unreachable: {stdout}"
    );
}

/// Pre-flight refuses whatever startup refuses, because both go through the
/// same loader — and a group claiming every `Ctrl` spelling of the quit chord
/// is not among them.
///
/// The compiled quit row matches every modifier set containing `Ctrl`, and two
/// of the six flags a terminal can report cannot be spelled in the grammar, so
/// `Ctrl+Hyper+C` still quits. Refusing this would refuse a configuration that
/// works. The refusal path is asserted where it can be reached, against the
/// loader directly.
#[test]
fn check_configuration_accepts_a_group_that_claims_every_spellable_quit_chord() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    let mut body = String::from("[bindings.global]\n");
    for spelling in [
        "ctrl",
        "ctrl+cmd",
        "ctrl+alt",
        "ctrl+shift",
        "ctrl+cmd+alt",
        "ctrl+cmd+shift",
        "ctrl+alt+shift",
        "ctrl+cmd+alt+shift",
    ] {
        body.push_str(&format!("\"{spelling}+c\" = \"none\"\n"));
    }
    fs::write(config_root.join("ui.toml"), body).expect("write ui.toml");

    let output = check(&[&config_root], &state_root);
    assert!(
        output.status.success(),
        "quit is still reachable, so pre-flight must not refuse: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The binding group resolves through the same effective-file lookup as the
/// rest of `ui.toml`, so a fault is reported against the copy in effect rather
/// than against one it shadows.
#[test]
fn check_configuration_reports_the_ui_toml_the_lookup_selected() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    // The shadowed copy is valid and configures nothing. Were the lookup to read
    // it instead, the run would succeed and report no finding, so this fails in
    // both directions rather than only on the path string.
    fs::write(config_root.join("ui.toml"), "default-bundle = \"alpha\"\n").expect("write base");
    let override_layer = temporary.path().join("override");
    fs::create_dir_all(&override_layer).expect("create override layer");
    fs::write(override_layer.join("ui.toml"), DISPLACES_SENDING).expect("write override");

    let output = check(&[&override_layer, &config_root], &state_root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains(&format!(
            "source ui.toml: {}",
            override_layer.join("ui.toml").display()
        )),
        "the reported source is not the copy in effect: {stdout}"
    );
    assert!(
        !stdout.contains(config_root.join("ui.toml").to_str().expect("base utf8")),
        "the shadowed copy must not appear as a source: {stdout}"
    );
    assert!(
        stdout.contains("binding finding:"),
        "the group in the selected copy was not the one inspected: {stdout}"
    );
}

/// Pre-flight is read-only, and so is the loader the TUI shares with it.
/// Nothing about reading a binding group may scaffold a copy of the compiled
/// defaults to disk, which would freeze today's table into an operator's
/// configuration and stop later releases reaching them.
#[test]
fn check_configuration_writes_no_configuration_artifact() {
    let temporary = TempDir::new().expect("temporary");
    let (config_root, state_root) = config_and_state(&temporary);
    write_bundle_configuration(&config_root, "alpha", None, &["a"]);
    fs::write(config_root.join("ui.toml"), DISPLACES_SENDING).expect("write ui.toml");

    let before = tree(&config_root);
    // Without this the comparison below would hold just as well over two empty
    // listings, which is what a `tree` that could not read the root would
    // produce.
    assert!(
        before.len() >= 2,
        "the fixture did not put a tree here to compare: {before:?}"
    );

    let output = check(&[&config_root], &state_root);
    assert!(output.status.success());
    assert_eq!(
        tree(&config_root),
        before,
        "pre-flight changed the configuration root"
    );
}

/// Every file under `root`, with its contents, so an added, removed, or
/// rewritten artifact all read as a difference.
///
/// Bounded to the temporary root this test created, and the tree is a handful
/// of files.
fn tree(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let contents = fs::read(&path).expect("read file");
                entries.push((path, contents));
            }
        }
    }
    entries.sort();
    entries
}
