use tempfile::TempDir;

use agentmux::configuration::{TargetConfiguration, load_bundle_configuration};

use super::helpers::*;

#[test]
fn loads_coder_environment_onto_member() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "acp"

[[coders.environment]]
name = "ACP_TOKEN"
value = "secret-123"

[[coders.environment]]
name = "LOG_LEVEL"
value = "debug"

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
"#,
            dir = temporary.path().display()
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 1);
    let TargetConfiguration::Acp(ref acp) = loaded.members[0].target else {
        panic!("expected ACP target configuration");
    };
    assert_eq!(acp.command.as_deref(), Some("opencode acp"));
    // The coder-level environment is lifted transport-agnostically onto the
    // resolved member, not held on the ACP-specific target descriptor.
    let environment = &loaded.members[0].environment;
    assert_eq!(environment.len(), 2);
    assert_eq!(environment[0].name, "ACP_TOKEN");
    assert_eq!(environment[0].value, "secret-123");
    assert_eq!(environment[1].name, "LOG_LEVEL");
    assert_eq!(environment[1].value, "debug");
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
fn rejects_acp_prime_timeout_ms_zero() {
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
prime-timeout-ms = 0
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
            .contains("ACP prime-timeout-ms must be greater than zero"),
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
    assert!(matches!(
        loaded.members[0].target,
        TargetConfiguration::Acp(_)
    ));
}

#[test]
fn loads_acp_prime_timeout_ms() {
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
prime-timeout-ms = 3210
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
    let TargetConfiguration::Acp(acp) = &loaded.members[0].target else {
        panic!("expected ACP target");
    };
    assert_eq!(acp.prime_timeout_ms, Some(3210));
}

/// The pre-existing legacy `turn-timeout-ms` key is no longer accepted; the raw
/// loader's `deny_unknown_fields` rejects it on bundle load. The renamed
/// `[coders.<id>.acp].prime-timeout-ms` key loads successfully.
#[test]
fn legacy_acp_turn_timeout_ms_is_rejected_with_deny_unknown_fields() {
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
turn-timeout-ms = 3210
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

    let err = load_bundle_configuration(&root, "alpha").expect_err("legacy key should fail");
    let rendered = err.to_string();
    assert!(
        rendered.contains("turn-timeout-ms") || rendered.contains("unknown field"),
        "expected the deny_unknown_fields error to mention the legacy key, got: {rendered}"
    );
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
    assert!(matches!(
        loaded.members[0].target,
        TargetConfiguration::Tmux(_)
    ));
    assert!(matches!(
        loaded.members[1].target,
        TargetConfiguration::Acp(_)
    ));
}

#[test]
fn pty_term_protocol_round_trips_each_variant() {
    use agentmux::configuration::TermProtocol;

    let cases: &[(&str, TermProtocol)] = &[
        ("xterm-256color", TermProtocol::Xterm256Color),
        ("xterm-kitty", TermProtocol::XtermKitty),
        ("alacritty", TermProtocol::Alacritty),
        ("foot", TermProtocol::Foot),
        ("wezterm", TermProtocol::WezTerm),
        ("screen-256color", TermProtocol::Screen256Color),
    ];
    for (term_str, expected) in cases {
        let temporary = TempDir::new().expect("temporary");
        let dir = temporary.path().display().to_string();
        let root = write_config(
            &temporary,
            "alpha",
            &format!(
                r#"
format-version = 1

[[coders]]
id = "alpha"

[coders.pty]
initial-command = "/bin/cat"
resume-command = "/bin/cat"
term-protocol = "{term_str}"
"#
            ),
            &format!(
                r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "alpha"
"#
            ),
        );
        let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
        let TargetConfiguration::Pty(ref pty) = loaded.members[0].target else {
            panic!("expected Pty target configuration for term-protocol = {term_str}");
        };
        assert_eq!(
            pty.term_protocol, *expected,
            "term-protocol = {term_str} should map to {expected:?}"
        );
    }
}

#[test]
fn pty_term_protocol_defaults_to_xterm_256color_when_absent() {
    use agentmux::configuration::TermProtocol;

    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "alpha"

[coders.pty]
initial-command = "/bin/cat"
resume-command = "/bin/cat"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "alpha"
"#
        ),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let TargetConfiguration::Pty(ref pty) = loaded.members[0].target else {
        panic!("expected Pty target configuration");
    };
    assert_eq!(pty.term_protocol, TermProtocol::Xterm256Color);
}

#[test]
fn pty_term_protocol_rejects_unknown_variant() {
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "alpha"

[coders.pty]
initial-command = "/bin/cat"
resume-command = "/bin/cat"
term-protocol = "xterm-kittie"
"#,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "a"
name = "a"
directory = "{dir}"
coder = "alpha"
"#
        ),
    );
    let result = load_bundle_configuration(&root, "alpha");
    assert!(
        result.is_err(),
        "unknown TermProtocol variant (typo xterm-kittie) should fail to load"
    );
}
