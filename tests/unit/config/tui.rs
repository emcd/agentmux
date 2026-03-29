use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{load_policy_ids, load_tui_configuration};

#[test]
fn loads_global_tui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("tui.toml"),
        r#"
default-bundle = "agentmux"
default-session = "user"

[[sessions]]
id = "user"
name = "Operator"
policy = "default"
"#,
    )
    .expect("write tui.toml");

    let loaded = load_tui_configuration(&root)
        .expect("load tui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_bundle.as_deref(), Some("agentmux"));
    assert_eq!(loaded.default_session.as_deref(), Some("user"));
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].id, "user");
    assert_eq!(loaded.sessions[0].policy_id, "default");
}

#[test]
fn ignores_missing_tui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    let loaded = load_tui_configuration(&root).expect("load tui config");
    assert!(loaded.is_none(), "missing file should be ignored");
}

#[test]
fn rejects_duplicate_tui_session_ids() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("tui.toml"),
        r#"
[[sessions]]
id = "user"
policy = "default"

[[sessions]]
id = "user"
policy = "default"
"#,
    )
    .expect("write tui.toml");

    let error = load_tui_configuration(&root).expect_err("duplicate selector should fail");
    assert!(error.to_string().contains("duplicate tui session id"));
}

#[test]
fn loads_policy_ids_from_policies_artifact() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("policies.toml"),
        r#"
format-version = 1

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "all:home"
send = "all:home"

[[policies]]
id = "restricted"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "none"
"#,
    )
    .expect("write policies.toml");

    let loaded = load_policy_ids(&root).expect("load policy ids");
    assert!(loaded.contains("default"));
    assert!(loaded.contains("restricted"));
}
