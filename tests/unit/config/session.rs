use tempfile::TempDir;

use agentmux::configuration::load_bundle_configuration;

use super::helpers::*;

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
fn rejects_session_id_starting_with_non_alpha() {
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
id = "9bad"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string()
            .contains("must start with an ASCII alphabetic character"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_session_id_with_invalid_characters() {
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
id = "bad.id"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string()
            .contains("may only contain ASCII alphanumeric"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_session_id_longer_than_31_characters() {
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
id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
directory = "{}"
coder = "shell"
"#,
            temporary.path().display()
        ),
    );

    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string().contains("exceeds max length 31"),
        "unexpected error: {err}"
    );
}

#[test]
fn session_type_capability_table_is_derived_from_transport() {
    use agentmux::configuration::SessionType;

    // Transport Capability Contract: (can_be_looked, can_be_written,
    // can_stream_output, gives_choices) per transport. `gives_choices`
    // describes choice production only; resolution authority is governed by
    // the `choose` policy capability.
    let table = [
        (SessionType::Tmux, (true, true, false, false)),
        (SessionType::Acp, (true, true, true, true)),
        (SessionType::Ui, (false, false, false, false)),
        (SessionType::Pubsub, (false, false, false, false)),
    ];
    for (session_type, (looked, written, streams, gives_choices)) in table {
        assert_eq!(session_type.can_be_looked(), looked, "{session_type:?}");
        assert_eq!(session_type.can_be_written(), written, "{session_type:?}");
        assert_eq!(
            session_type.can_stream_output(),
            streams,
            "{session_type:?}"
        );
        assert_eq!(
            session_type.can_give_choices(),
            gives_choices,
            "{session_type:?}"
        );
    }
}
