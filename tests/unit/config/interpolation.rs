use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{TargetConfiguration, load_bundle_configuration};

#[cfg(feature = "pty")]
use agentmux::pty::tokenize_command;

use super::helpers::*;

const CODERS_TMUX: &str = r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = "cistella conduct --session-directory {{session-directory}} --label agentmux.session={{bundle-session-id}} -- opencode"
resume-command = "cistella conduct --session-directory {{session-directory}} --label agentmux.session={{bundle-session-id}} --resume {{coder-session-id}} -- opencode"
"#;

const CODERS_PTY: &str = r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.pty]
initial-command = "cistella conduct --session-directory {{session-directory}} --label agentmux.session={{bundle-session-id}} -- opencode"
resume-command = "cistella conduct --session-directory {{session-directory}} --label agentmux.session={{bundle-session-id}} --resume {{coder-session-id}} -- opencode"
"#;

fn session_toml(directory: &str, coder_session_id: Option<&str>) -> String {
    let resume = coder_session_id
        .map(|value| format!("coder-session-id = \"{value}\"\n"))
        .unwrap_or_default();
    let escaped = directory.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"
format-version = 1

[[sessions]]
id = "session-a"
name = "a"
directory = "{escaped}"
coder = "conduct"
{resume}"#
    )
}

fn start_command_for(coders_toml: &str, directory: &str, coder_session_id: Option<&str>) -> String {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        coders_toml,
        &session_toml(directory, coder_session_id),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 1);
    match &loaded.members[0].target {
        TargetConfiguration::Tmux(target) => target.start_command.clone(),
        TargetConfiguration::Pty(target) => target.initial_command.clone(),
        other => panic!("expected spawning target, got {other:?}"),
    }
}

#[test]
fn tmux_initial_substitutes_session_variables() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = start_command_for(CODERS_TMUX, &directory, None);
    assert_eq!(
        command.as_str(),
        &format!(
            "cistella conduct --session-directory '{directory}' \
             --label agentmux.session=session-a -- opencode"
        )
    );
}

#[test]
fn tmux_resume_substitutes_all_variables() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = start_command_for(CODERS_TMUX, &directory, Some("abc123"));
    assert_eq!(
        command.as_str(),
        &format!(
            "cistella conduct --session-directory '{directory}' \
             --label agentmux.session=session-a --resume abc123 -- opencode"
        )
    );
}

#[test]
fn pty_initial_substitutes_session_variables() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = start_command_for(CODERS_PTY, &directory, None);
    assert_eq!(
        command.as_str(),
        &format!(
            "cistella conduct --session-directory '{directory}' \
             --label agentmux.session=session-a -- opencode"
        )
    );
}

#[test]
fn pty_resume_substitutes_all_variables() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = start_command_for(CODERS_PTY, &directory, Some("abc123"));
    assert_eq!(
        command.as_str(),
        &format!(
            "cistella conduct --session-directory '{directory}' \
             --label agentmux.session=session-a --resume abc123 -- opencode"
        )
    );
}

#[test]
fn resolves_brace_shaped_directory_without_false_rejection() {
    let temporary = TempDir::new().expect("temporary");
    let shaped = temporary.path().join("{name}");
    fs::create_dir(&shaped).expect("create brace directory");
    let directory = shaped.display().to_string();
    let command = start_command_for(CODERS_TMUX, &directory, None);
    assert!(
        command.contains(&format!("'{directory}'")),
        "brace-shaped directory must render quoted, got: {command}"
    );
}

fn command_for_initial(
    initial_command: &str,
    directory: &str,
    coder_session_id: Option<&str>,
) -> String {
    command_for_commands(initial_command, "\"conduct\"", directory, coder_session_id)
}

fn command_for_commands(
    initial_command: &str,
    resume_command: &str,
    directory: &str,
    coder_session_id: Option<&str>,
) -> String {
    let temporary = TempDir::new().expect("temporary");
    let coders = format!(
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = {initial_command}
resume-command = {resume_command}
"#
    );
    let root = write_config(
        &temporary,
        "alpha",
        &coders,
        &session_toml(directory, coder_session_id),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let TargetConfiguration::Tmux(target) = &loaded.members[0].target else {
        panic!("expected tmux target");
    };
    target.start_command.clone()
}

fn load_error_for_initial(initial_command: &str, directory: &str) -> String {
    let temporary = TempDir::new().expect("temporary");
    let coders = format!(
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = {initial_command}
resume-command = "conduct"
"#
    );
    let root = write_config(&temporary, "alpha", &coders, &session_toml(directory, None));
    load_bundle_configuration(&root, "alpha")
        .expect_err("misplaced placeholder loads")
        .to_string()
}

#[test]
fn rejects_escaped_unknown_placeholder() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("\"cmd \\\\{{bundle}}\"", &directory);
    assert!(
        error.contains("unknown placeholder"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_escaped_directory_placeholder() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("\"cmd \\\\{{session-directory}}\"", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn escaped_id_token_still_substitutes_raw() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_initial("\"conduct \\\\{{bundle-session-id}}\"", &directory, None);
    assert!(
        command.contains("\\session-a"),
        "escaped id token must still substitute, got: {command}"
    );
}

#[test]
fn rejects_directory_inside_single_quotes() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("\"cmd '{{session-directory}}'\"", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_directory_inside_double_quotes() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("\"cmd \\\"{{session-directory}}\\\"\"", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_directory_with_adjacent_prefix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("'cmd prefix{{session-directory}}'", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_directory_with_adjacent_suffix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("'cmd {{session-directory}}suffix'", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_directory_after_escape_continuing_word() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_initial("'cmd \\\"{{session-directory}}'", &directory);
    assert!(
        error.contains("standalone unquoted word"),
        "unexpected error: {error}"
    );
}

#[test]
fn accepts_directory_after_escape_then_whitespace() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_initial("'cmd \\\" {{session-directory}}'", &directory, None);
    assert!(
        command.contains(&format!("'{directory}'")),
        "expected quoted directory, got: {command}"
    );
}

#[test]
fn quoted_id_tokens_stay_valid() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let quoted = command_for_commands(
        "\"conduct\"",
        "\"conduct --resume '{{coder-session-id}}' --label \\\"{{bundle-session-id}}\\\"\"",
        &directory,
        Some("abc123"),
    );
    assert!(
        quoted.contains("--resume 'abc123' --label \"session-a\""),
        "quoted id tokens must resolve, got: {quoted}"
    );
}

#[test]
fn rejects_single_brace_coder_session_id_as_unknown() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = "conduct --resume {coder-session-id}"
resume-command = "conduct"
"#,
        &session_toml(&directory, None),
    );
    let error = load_bundle_configuration(&root, "alpha").expect_err("single-brace loads");
    assert!(
        error
            .to_string()
            .contains("unknown placeholder '{coder-session-id}'"),
        "unexpected error: {error}"
    );
}

const SPECIAL_DIRECTORY_NAME: &str = "two words'o\\clock";

#[cfg(feature = "pty")]
#[test]
fn pty_tokenizer_returns_special_directory_as_single_argument() {
    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = start_command_for(CODERS_TMUX, &directory, None);
    let tokens = tokenize_command(&command).expect("tokenize rendered command");
    let flag = tokens
        .iter()
        .position(|token| token == "--session-directory")
        .expect("session-directory flag present");
    assert_eq!(tokens[flag + 1], directory);
}

#[cfg(unix)]
#[test]
fn shell_receives_special_directory_as_single_argument() {
    use std::process::Command;

    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = command_for_initial("\"printf '<%s>' {{session-directory}}\"", &directory, None);
    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .expect("run shell seam");
    assert!(output.status.success(), "shell failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("<{directory}>")
    );
}

#[test]
fn quotes_directory_with_command_significant_characters() {
    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join("two words'quoted");
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = start_command_for(CODERS_TMUX, &directory, None);
    let quoted = format!("'{}'", directory.replace('\'', "'\\''"));
    assert!(
        command.contains(&quoted),
        "expected single-quoted directory, got: {command}"
    );
}

#[test]
fn rejects_unknown_double_brace_placeholder() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = "conduct --label agentmux.bundle={{bundle}}"
resume-command = "conduct"
"#,
        &session_toml(&directory, None),
    );
    let error = load_bundle_configuration(&root, "alpha").expect_err("unknown placeholder loads");
    assert!(
        error
            .to_string()
            .contains("unknown placeholder '{{bundle}}'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_unknown_placeholder_with_brace_shaped_directory() {
    let temporary = TempDir::new().expect("temporary");
    let shaped = temporary.path().join("{name}");
    fs::create_dir(&shaped).expect("create brace directory");
    let directory = shaped.display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = "conduct --label agentmux.bundle={{bundle}}"
resume-command = "conduct"
"#,
        &session_toml(&directory, None),
    );
    let error = load_bundle_configuration(&root, "alpha").expect_err("unknown placeholder loads");
    assert!(
        error
            .to_string()
            .contains("unknown placeholder '{{bundle}}'"),
        "unexpected error: {error}"
    );
}

#[test]
fn acp_stdio_command_with_braces_passes_through_verbatim() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command_text =
        "cistella conduct --label agentmux.session={{bundle-session-id}} -- {harness}";
    let root = write_config(
        &temporary,
        "alpha",
        &format!(
            r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.acp]
channel = "stdio"
command = "{command_text}"
"#
        ),
        &session_toml(&directory, None),
    );
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let TargetConfiguration::Acp(target) = &loaded.members[0].target else {
        panic!("expected acp target");
    };
    assert_eq!(target.command.as_deref(), Some(command_text));
}
