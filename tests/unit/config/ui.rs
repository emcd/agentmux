use agentmux::configuration::ConfigurationRoots;
use std::fs;

use tempfile::TempDir;

use agentmux::configuration::load_ui_configuration;

#[test]
fn loads_default_bundle_from_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("ui.toml"),
        r#"
default-bundle = "agentmux"
"#,
    )
    .expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_bundle.as_deref(), Some("agentmux"));
}

#[test]
fn ignores_missing_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root)).expect("load ui config");
    assert!(loaded.is_none(), "missing file should be ignored");
}

#[test]
fn empty_ui_configuration_resolves_no_default_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "").expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui config")
        .expect("existing config");
    assert!(loaded.default_bundle.is_none());
}

#[test]
fn rejects_malformed_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "default-bundle = ").expect("write ui.toml");

    let error = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect_err("malformed ui.toml should fail");
    assert!(
        error.to_string().contains("ui.toml"),
        "error should name the offending file: {error}"
    );
}
