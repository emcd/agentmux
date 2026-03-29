use std::fs;

use agentmux::runtime::starter::ensure_starter_configuration_layout;
use tempfile::TempDir;

#[test]
fn creates_starter_configuration_files_when_missing() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");

    ensure_starter_configuration_layout(&configuration_root).expect("starter layout");

    let coders = configuration_root.join("coders.toml");
    let policies = configuration_root.join("policies.toml");
    let tui = configuration_root.join("tui.toml");
    let example_bundle = configuration_root.join("bundles/example.toml");
    assert!(coders.exists(), "expected coders.toml to exist");
    assert!(policies.exists(), "expected policies.toml to exist");
    assert!(tui.exists(), "expected tui.toml to exist");
    assert!(example_bundle.exists(), "expected example bundle to exist");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert!(coders_text.contains("format-version = 1"));
    assert!(coders_text.contains("[[coders]]"));
    let policies_text = fs::read_to_string(policies).expect("read policies.toml");
    assert!(policies_text.contains("format-version = 1"));
    assert!(policies_text.contains("[[policies]]"));
    let tui_text = fs::read_to_string(tui).expect("read tui.toml");
    assert!(tui_text.contains("default-bundle"));
    assert!(tui_text.contains("[[sessions]]"));

    let bundle_text = fs::read_to_string(example_bundle).expect("read example bundle");
    assert!(bundle_text.contains("format-version = 1"));
    assert!(bundle_text.contains("[[sessions]]"));
}

#[test]
fn preserves_existing_configuration_files() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");
    fs::create_dir_all(configuration_root.join("bundles")).expect("create bundle dir");
    let coders = configuration_root.join("coders.toml");
    let policies = configuration_root.join("policies.toml");
    let tui = configuration_root.join("tui.toml");
    let example_bundle = configuration_root.join("bundles/example.toml");
    fs::write(&coders, "format-version = 1\n# custom coders\n").expect("write coders");
    fs::write(&policies, "format-version = 1\n# custom policies\n").expect("write policies");
    fs::write(&tui, "default-bundle = \"custom\"\n# custom tui\n").expect("write tui");
    fs::write(&example_bundle, "format-version = 1\n# custom bundle\n").expect("write bundle");

    ensure_starter_configuration_layout(&configuration_root).expect("starter layout");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert_eq!(coders_text, "format-version = 1\n# custom coders\n");
    let policies_text = fs::read_to_string(policies).expect("read policies.toml");
    assert_eq!(policies_text, "format-version = 1\n# custom policies\n");
    let tui_text = fs::read_to_string(tui).expect("read tui.toml");
    assert_eq!(tui_text, "default-bundle = \"custom\"\n# custom tui\n");
    let bundle_text = fs::read_to_string(example_bundle).expect("read example bundle");
    assert_eq!(bundle_text, "format-version = 1\n# custom bundle\n");
}
