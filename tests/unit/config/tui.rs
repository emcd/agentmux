use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{load_policy_ids, load_tui_configuration};

#[test]
fn loads_global_tui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("users.toml"),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
name = "Operator"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write users.toml");

    let loaded = load_tui_configuration(&root)
        .expect("load tui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_session.as_deref(), Some("user@GLOBAL"));
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].id, "user@GLOBAL");
    assert_eq!(loaded.sessions[0].policy, "default");
}

#[test]
fn normalizes_bare_session_ids_to_global_form() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("users.toml"),
        r#"
default-session = "user"

[[sessions]]
id = "user"
name = "Operator"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write users.toml");

    let loaded = load_tui_configuration(&root)
        .expect("load tui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_session.as_deref(), Some("user@GLOBAL"));
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].id, "user@GLOBAL");
}

#[test]
fn detects_duplicates_across_bare_and_suffixed_session_ids() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("users.toml"),
        r#"
[[sessions]]
id = "user"
policy = "default"

[sessions.ui]

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write users.toml");

    let error = load_tui_configuration(&root).expect_err("duplicate selector should fail");
    assert!(error.to_string().contains("duplicate users session id"));
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
        root.join("users.toml"),
        r#"
[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write users.toml");

    let error = load_tui_configuration(&root).expect_err("duplicate selector should fail");
    assert!(error.to_string().contains("duplicate users session id"));
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
list = "home"
look = "home"
send = "home"

[[policies]]
id = "restricted"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "none"
"#,
    )
    .expect("write policies.toml");

    let loaded = load_policy_ids(&root).expect("load policy ids");
    assert!(loaded.contains("default"));
    assert!(loaded.contains("restricted"));
}
