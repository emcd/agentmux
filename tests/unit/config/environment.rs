//! Coder/bundle/session environment merge, precedence, and validation.
//!
//! Environment is declared at three layers and merged once at configuration
//! load onto each resolved [`BundleMember`]'s `environment`, with per-variable
//! precedence session > bundle > coder and a union of distinct names.

use tempfile::TempDir;

use agentmux::configuration::{BringUpContext, BundleMember, load_bundle_configuration};

use super::helpers::*;

/// Looks up a merged environment value by name on a resolved member.
fn value_of<'member>(member: &'member BundleMember, name: &str) -> Option<&'member str> {
    member
        .environment
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value.as_str())
}

/// A coder that declares two coder-level environment variables, plus a bundle
/// file that layers bundle- and session-level environment over it, exercising
/// the full session > bundle > coder precedence stack and the union of names
/// declared at a single level.
fn write_layered_environment(temporary: &TempDir) -> std::path::PathBuf {
    write_config(
        temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[[coders.environment]]
name = "SHARED"
value = "coder"

[[coders.environment]]
name = "ONLY_CODER"
value = "c"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[environment]]
name = "SHARED"
value = "bundle"

[[environment]]
name = "ONLY_BUNDLE"
value = "b"

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "acp"

[[sessions.environment]]
name = "SHARED"
value = "session"

[[sessions.environment]]
name = "ONLY_SESSION"
value = "s"
"#,
            dir = temporary.path().display()
        ),
    )
}

#[test]
fn session_value_overrides_bundle_and_coder() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_layered_environment(&temporary);

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let member = &loaded.members[0];

    // Most-specific level wins for a name declared at all three layers.
    assert_eq!(value_of(member, "SHARED"), Some("session"));
}

#[test]
fn distinct_names_union_across_levels() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_layered_environment(&temporary);

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let member = &loaded.members[0];

    // Names declared at only one level all survive the merge.
    assert_eq!(value_of(member, "ONLY_CODER"), Some("c"));
    assert_eq!(value_of(member, "ONLY_BUNDLE"), Some("b"));
    assert_eq!(value_of(member, "ONLY_SESSION"), Some("s"));
    // SHARED collapses to a single entry: four distinct declared names, plus
    // the bring-up context stamped after them.
    assert_eq!(
        member.environment.len(),
        4 + BringUpContext::VARIABLE_NAMES.len()
    );
    // Declaration order is deterministic: first-appearance position is kept
    // (coder names, then new bundle names, then new session names). The stamped
    // context trails the declared layers and is asserted separately.
    let names: Vec<&str> = member.environment[..4]
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["SHARED", "ONLY_CODER", "ONLY_BUNDLE", "ONLY_SESSION"]
    );
}

#[test]
fn bundle_value_overrides_coder_when_session_absent() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[[coders.environment]]
name = "SHARED"
value = "coder"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[environment]]
name = "SHARED"
value = "bundle"

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "acp"
"#,
            dir = temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(value_of(&loaded.members[0], "SHARED"), Some("bundle"));
}

#[test]
fn bundle_environment_applies_to_every_member() {
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[environment]]
name = "OPENCODE_ENABLE_EXA"
value = "1"

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "acp"

[[sessions]]
id = "b"
name = "b"
directory = "{dir}"
coder = "acp"
"#
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 2);
    for member in &loaded.members {
        assert_eq!(value_of(member, "OPENCODE_ENABLE_EXA"), Some("1"));
    }
}

#[test]
fn session_environment_lands_on_its_member_only() {
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "acp"

[[sessions.environment]]
name = "PER_SESSION"
value = "yes"

[[sessions]]
id = "b"
name = "b"
directory = "{dir}"
coder = "acp"
"#
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(value_of(&loaded.members[0], "PER_SESSION"), Some("yes"));
    // Member b carries the stamped context but none of a's declared entries.
    assert_eq!(value_of(&loaded.members[1], "PER_SESSION"), None);
}

#[test]
fn rejects_coder_environment_name_with_equals() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[[coders.environment]]
name = "BAD=KEY"
value = "x"

[coders.acp]
channel = "stdio"
command = "opencode acp"
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

    // An environment name is an OS env-var key; '=' is unrepresentable and must
    // fail at load, not panic at spawn (`Command::env`) or produce a malformed
    // tmux `-e BAD=KEY=x`.
    let err = load_bundle_configuration(&root, "alpha").expect_err("load should fail");
    assert!(
        err.to_string()
            .contains("coder 'acp' environment entry 0 name 'BAD=KEY' must not contain '=' or NUL"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_empty_bundle_environment_name() {
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
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[environment]]
name = ""
value = "orphan"

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
            .contains("bundle 'alpha' environment entry 0 has empty name"),
        "unexpected error: {err}"
    );
}

#[test]
fn allows_empty_session_environment_value() {
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
command = "opencode acp"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{}"
coder = "acp"

[[sessions.environment]]
name = "EMPTY_OK"
value = ""
"#,
            temporary.path().display()
        ),
    );

    // An empty string is a legitimate OS environment value; only a *missing*
    // value field is rejected (by serde, as the field is required).
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(value_of(&loaded.members[0], "EMPTY_OK"), Some(""));
}
