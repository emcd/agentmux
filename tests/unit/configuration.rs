use std::{fs, path::PathBuf};

use tempfile::TempDir;
use tmuxmux::configuration::{
    ConfigurationError, infer_sender_from_working_directory, load_bundle_configuration,
};

fn write_bundle(temporary: &TempDir, bundle_name: &str, content: &str) -> PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    fs::create_dir_all(&bundles).expect("create directories");
    let path = bundles.join(format!("{bundle_name}.json"));
    fs::write(&path, content).expect("write config");
    root
}

#[test]
fn loads_valid_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_bundle(
        &temporary,
        "alpha",
        r#"{
            "schema_version": "1",
            "members": [
                {"session_name": "a"},
                {"session_name": "b", "display_name": "Bravo"}
            ]
        }"#,
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.bundle_name, "alpha");
    assert_eq!(loaded.members.len(), 2);
    assert_eq!(loaded.members[1].display_name.as_deref(), Some("Bravo"));
}

#[test]
fn rejects_duplicate_session_names() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_bundle(
        &temporary,
        "alpha",
        r#"{
            "members": [
                {"session_name": "dup"},
                {"session_name": "dup"}
            ]
        }"#,
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("duplicate session_name"),
        "unexpected error: {err}"
    );
}

#[test]
fn reports_unknown_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    let err = load_bundle_configuration(&root, "missing").expect_err("missing bundle");
    match err {
        ConfigurationError::UnknownBundle { bundle_name, .. } => {
            assert_eq!(bundle_name, "missing");
        }
        _ => panic!("expected unknown bundle"),
    }
}

#[test]
fn infers_sender_from_matching_working_directory() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_bundle(
        &temporary,
        "alpha",
        &format!(
            r#"{{
            "members": [
                {{"session_name": "a", "working_directory": "{}"}},
                {{"session_name": "b", "working_directory": "{}"}}
            ]
        }}"#,
            temporary.path().display(),
            temporary.path().join("other").display()
        ),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load");

    let inferred =
        infer_sender_from_working_directory(&loaded, temporary.path()).expect("infer sender");
    assert_eq!(inferred.as_deref(), Some("a"));
}
