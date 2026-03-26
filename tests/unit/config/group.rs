use std::fs;

use tempfile::TempDir;

use agentmux::configuration::load_bundle_configuration;

use super::helpers::*;

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
autostart = true
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

    let memberships =
        agentmux::configuration::load_bundle_group_memberships(&root).expect("load memberships");
    assert_eq!(memberships.len(), 2);
    assert_eq!(memberships[0].bundle_name, "alpha");
    assert!(memberships[0].autostart);
    assert_eq!(memberships[0].groups, vec!["dev".to_string()]);
    assert_eq!(memberships[1].bundle_name, "bravo");
    assert!(!memberships[1].autostart);
    assert!(memberships[1].groups.is_empty());
}
