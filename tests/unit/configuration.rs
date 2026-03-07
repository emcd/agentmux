use std::{fs, path::PathBuf};

use agentmux::configuration::{
    ConfigurationError, infer_sender_from_working_directory, load_bundle_configuration,
};
use tempfile::TempDir;

fn write_config(
    temporary: &TempDir,
    bundle_name: &str,
    coders_toml: &str,
    bundle_toml: &str,
) -> PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    fs::create_dir_all(&bundles).expect("create directories");
    fs::write(root.join("coders.toml"), coders_toml).expect("write coders");
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml).expect("write bundle");
    root
}

#[test]
fn loads_valid_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "codex"
initial-command = "codex start"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "^›"
prompt-inspect-lines = 8
prompt-idle-column = 3

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "session-a"
name = "a"
directory = "{}"
coder = "codex"
coder-session-id = "abc123"

[[sessions]]
id = "session-b"
name = "Bravo"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display(),
            temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.bundle_name, "alpha");
    assert_eq!(loaded.members.len(), 2);
    assert_eq!(loaded.members[1].id, "session-b");
    assert_eq!(loaded.members[1].name.as_deref(), Some("Bravo"));
    assert_eq!(
        loaded.members[0].start_command.as_deref(),
        Some("codex resume abc123")
    );
    let readiness = loaded.members[0]
        .prompt_readiness
        .as_ref()
        .expect("member a prompt_readiness");
    assert_eq!(readiness.prompt_regex, "^›");
    assert_eq!(readiness.inspect_lines, Some(8));
    assert_eq!(readiness.input_idle_cursor_column, Some(3));
}

#[test]
fn rejects_duplicate_session_names() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "one"
name = "dup"
directory = "{}"
coder = "shell"

[[sessions]]
id = "two"
name = "dup"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display(),
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("duplicate session name"),
        "unexpected error: {err}"
    );
}

#[test]
fn reports_unknown_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders");

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
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"

[[sessions]]
id = "b"
name = "b"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display(),
            temporary.path().join("other").display()
        ),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load");

    let inferred =
        infer_sender_from_working_directory(&loaded, temporary.path()).expect("infer sender");
    assert_eq!(inferred.as_deref(), Some("a"));
}

#[test]
fn rejects_ambiguous_sender_from_working_directory() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"

[[sessions]]
id = "b"
name = "b"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display(),
            temporary.path().display()
        ),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load");

    let err = infer_sender_from_working_directory(&loaded, temporary.path())
        .expect_err("ambiguous sender should fail");
    match err {
        ConfigurationError::AmbiguousSender { matches, .. } => {
            assert_eq!(matches, vec!["a".to_string(), "b".to_string()]);
        }
        _ => panic!("expected ambiguous sender error"),
    }
}

#[test]
fn rejects_invalid_prompt_regex() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
prompt-regex = "["
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("prompt-regex"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_zero_prompt_inspect_lines() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
prompt-regex = "^ok$"
prompt-inspect-lines = 0
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("prompt-inspect-lines"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unknown_coder_reference() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "missing"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("unknown coder"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_duplicate_coder_ids() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "dup"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[[coders]]
id = "dup"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "dup"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("duplicate coder id"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unknown_command_placeholder() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
initial-command = "echo {unknown-token}"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("unknown placeholder"),
        "unexpected error: {err}"
    );
}
