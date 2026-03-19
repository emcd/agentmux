use std::path::Path;

use agentmux::{
    configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
    },
    runtime::tui_sender::{
        load_local_tui_override_sender, load_tui_configuration_sender, resolve_tui_sender_session,
    },
};
use tempfile::TempDir;

fn bundle_with_sessions(sessions: &[&str]) -> BundleConfiguration {
    BundleConfiguration {
        schema_version: "1".to_string(),
        bundle_name: "agentmux".to_string(),
        groups: Vec::new(),
        members: sessions
            .iter()
            .map(|session| BundleMember {
                id: (*session).to_string(),
                name: None,
                working_directory: None,
                target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
                    start_command: "sh -lc 'true'".to_string(),
                    prompt_readiness: None,
                }),
                coder_session_id: None,
                policy_id: None,
            })
            .collect(),
    }
}

#[test]
fn resolves_cli_sender_before_other_sources() {
    let bundle = bundle_with_sessions(&["cli", "override", "config", "assoc"]);
    let resolved = resolve_tui_sender_session(
        &bundle,
        Path::new("/tmp"),
        "assoc",
        Some("cli"),
        Some("override"),
        Some("config"),
    )
    .expect("resolve sender");
    assert_eq!(resolved, "cli");
}

#[test]
fn resolves_override_sender_before_config_and_association() {
    let bundle = bundle_with_sessions(&["override", "config", "assoc"]);
    let resolved = resolve_tui_sender_session(
        &bundle,
        Path::new("/tmp"),
        "assoc",
        None,
        Some("override"),
        Some("config"),
    )
    .expect("resolve sender");
    assert_eq!(resolved, "override");
}

#[test]
fn resolves_configuration_sender_before_association() {
    let bundle = bundle_with_sessions(&["config", "assoc"]);
    let resolved = resolve_tui_sender_session(
        &bundle,
        Path::new("/tmp"),
        "assoc",
        None,
        None,
        Some("config"),
    )
    .expect("resolve sender");
    assert_eq!(resolved, "config");
}

#[test]
fn falls_back_to_association_sender() {
    let bundle = bundle_with_sessions(&["assoc"]);
    let resolved =
        resolve_tui_sender_session(&bundle, Path::new("/tmp"), "assoc", None, None, None)
            .expect("resolve sender");
    assert_eq!(resolved, "assoc");
}

#[test]
fn rejects_unknown_cli_sender_without_fallback() {
    let bundle = bundle_with_sessions(&["assoc"]);
    let error = resolve_tui_sender_session(
        &bundle,
        Path::new("/tmp"),
        "assoc",
        Some("ghost"),
        None,
        None,
    )
    .expect_err("must fail");
    assert!(error.to_string().contains("validation_unknown_sender"));
}

#[test]
fn loads_configuration_sender_when_file_exists() {
    let temporary = TempDir::new().expect("temporary");
    std::fs::write(temporary.path().join("tui.toml"), "sender = 'tui'\n").expect("write config");

    let loaded =
        load_tui_configuration_sender(temporary.path()).expect("load configuration sender");
    assert_eq!(loaded.as_deref(), Some("tui"));
}

#[test]
fn ignores_missing_configuration_sender_file() {
    let temporary = TempDir::new().expect("temporary");
    let loaded =
        load_tui_configuration_sender(temporary.path()).expect("load configuration sender");
    assert_eq!(loaded, None);
}

#[test]
fn rejects_malformed_configuration_sender_file() {
    let temporary = TempDir::new().expect("temporary");
    std::fs::write(temporary.path().join("tui.toml"), "unknown = 1\n").expect("write config");

    let error = load_tui_configuration_sender(temporary.path()).expect_err("must fail");
    assert!(error.to_string().contains("validation_invalid_arguments"));
}

#[test]
fn rejects_empty_configuration_sender_field() {
    let temporary = TempDir::new().expect("temporary");
    std::fs::write(temporary.path().join("tui.toml"), "sender = '   '\n").expect("write config");

    let error = load_tui_configuration_sender(temporary.path()).expect_err("must fail");
    assert!(error.to_string().contains("validation_invalid_arguments"));
}

#[test]
fn loads_local_override_sender_when_present() {
    let temporary = TempDir::new().expect("temporary");
    let override_directory = temporary
        .path()
        .join(".auxiliary/configuration/agentmux/overrides");
    std::fs::create_dir_all(&override_directory).expect("create override directory");
    std::fs::write(override_directory.join("tui.toml"), "sender = 'tui'\n")
        .expect("write override");

    let loaded = load_local_tui_override_sender(temporary.path()).expect("load override sender");
    if cfg!(debug_assertions) {
        assert_eq!(loaded.as_deref(), Some("tui"));
    } else {
        assert_eq!(loaded, None);
    }
}
