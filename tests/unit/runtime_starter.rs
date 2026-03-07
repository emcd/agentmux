use std::fs;

use agentmux::runtime::starter::ensure_starter_configuration_layout;
use tempfile::TempDir;

#[test]
fn creates_starter_configuration_files_when_missing() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");

    ensure_starter_configuration_layout(&configuration_root).expect("starter layout");

    let coders = configuration_root.join("coders.toml");
    let example_bundle = configuration_root.join("bundles/example.toml");
    assert!(coders.exists(), "expected coders.toml to exist");
    assert!(example_bundle.exists(), "expected example bundle to exist");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert!(coders_text.contains("format-version = 1"));
    assert!(coders_text.contains("[[coders]]"));

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
    let example_bundle = configuration_root.join("bundles/example.toml");
    fs::write(&coders, "format-version = 1\n# custom coders\n").expect("write coders");
    fs::write(&example_bundle, "format-version = 1\n# custom bundle\n").expect("write bundle");

    ensure_starter_configuration_layout(&configuration_root).expect("starter layout");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert_eq!(coders_text, "format-version = 1\n# custom coders\n");
    let bundle_text = fs::read_to_string(example_bundle).expect("read example bundle");
    assert_eq!(bundle_text, "format-version = 1\n# custom bundle\n");
}
