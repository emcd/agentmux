use std::{fs, path::PathBuf};

use agentmux::configuration::{
    ConfigurationError, infer_sender_from_working_directory, load_bundle_configuration,
    load_bundle_group_memberships,
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

[coders.tmux]
initial-command = "codex start"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "^›"
prompt-inspect-lines = 8
prompt-idle-column = 3

[[coders]]
id = "shell"

[coders.tmux]
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
policy = "default"

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
    assert_eq!(loaded.members[0].policy_id.as_deref(), Some("default"));
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
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

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[[coders]]
id = "dup"

[coders.tmux]
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

[coders.tmux]
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

#[test]
fn loads_bundle_configuration_with_custom_groups() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1
groups = ["dev", "login"]

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.groups, vec!["dev".to_string(), "login".to_string()]);
}

#[test]
fn rejects_reserved_all_group_in_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1
groups = ["ALL"]

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
        err.to_string().contains("reserved"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_uppercase_custom_group_in_bundle_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
        &format!(
            r#"
format-version = 1
groups = ["DEV"]

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
        err.to_string().contains("invalid group name"),
        "unexpected error: {err}"
    );
}

#[test]
fn loads_group_memberships_with_optional_groups_key() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    fs::create_dir_all(&bundles).expect("create directories");
    fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders");
    fs::write(
        bundles.join("alpha.toml"),
        format!(
            r#"
format-version = 1
groups = ["dev"]

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    )
    .expect("write alpha");
    fs::write(
        bundles.join("bravo.toml"),
        format!(
            r#"
format-version = 1

[[sessions]]
id = "b"
name = "b"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    )
    .expect("write bravo");

    let memberships = load_bundle_group_memberships(&root).expect("load memberships");
    assert_eq!(memberships.len(), 2);
    assert_eq!(memberships[0].bundle_name, "alpha");
    assert_eq!(memberships[0].groups, vec!["dev".to_string()]);
    assert_eq!(memberships[1].bundle_name, "bravo");
    assert!(memberships[1].groups.is_empty());
}

#[test]
fn rejects_missing_coder_target_descriptor() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"
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
        err.to_string().contains("exactly one target table"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_multiple_coder_target_descriptors() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[coders.acp]
channel = "stdio"
command = "acp-shell"
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
        err.to_string().contains("multiple target tables"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_acp_stdio_without_command() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "acp"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string()
            .contains("stdio target requires non-empty command"),
        "unexpected error: {err}"
    );
}

#[test]
fn allows_acp_session_without_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "acp-shell"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "acp"
"#,
            temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 1);
    assert_eq!(loaded.members[0].coder_session_id, None);
    assert!(loaded.members[0].acp.is_some());
}

#[test]
fn loads_mixed_tmux_and_acp_coder_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "tmux"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "acp-shell"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "tmux"

[[sessions]]
id = "b"
name = "b"
directory = "{}"
coder = "acp"
"#,
            temporary.path().display(),
            temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 2);
    assert!(loaded.members[0].start_command.is_some());
    assert!(loaded.members[1].start_command.is_none());
    assert!(loaded.members[1].acp.is_some());
}

#[test]
fn rejects_unsupported_format_version() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 3

[[coders]]
id = "shell"

[coders.tmux]
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
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("unsupported format-version"),
        "unexpected error: {err}"
    );
}
