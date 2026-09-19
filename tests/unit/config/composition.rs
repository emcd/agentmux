use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{TargetConfiguration, load_bundle_configuration};

#[cfg(feature = "pty")]
use agentmux::pty::tokenize_command;

use super::helpers::*;

fn tmux_coders(initial_command: &str, resume_command: &str) -> String {
    format!(
        r#"
format-version = 1

[[coders]]
id = "conduct"

[coders.tmux]
initial-command = {initial_command}
resume-command = {resume_command}
"#
    )
}

fn bundle_document(
    directory: &str,
    coder_session_id: Option<&str>,
    session_extra: &str,
    bundle_extra: &str,
) -> String {
    let resume = coder_session_id
        .map(|value| format!("coder-session-id = \"{value}\"\n"))
        .unwrap_or_default();
    let escaped = directory.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"
format-version = 1
{bundle_extra}
[[sessions]]
id = "session-a"
name = "a"
directory = "{escaped}"
coder = "conduct"
{resume}{session_extra}"#
    )
}

fn start_command(bundle_name: &str, coders_toml: &str, bundle_toml: &str) -> String {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(&temporary, bundle_name, coders_toml, bundle_toml);
    let loaded = load_bundle_configuration(&root, bundle_name).expect("load configuration");
    assert_eq!(loaded.members.len(), 1);
    match &loaded.members[0].target {
        TargetConfiguration::Tmux(target) => target.start_command.clone(),
        TargetConfiguration::Pty(target) => target.initial_command.clone(),
        other => panic!("expected spawning target, got {other:?}"),
    }
}

fn command_for_template(
    initial_command: &str,
    directory: &str,
    session_extra: &str,
    bundle_extra: &str,
) -> String {
    let coders = tmux_coders(initial_command, "\"conduct\"");
    let bundle = bundle_document(directory, None, session_extra, bundle_extra);
    start_command("alpha", &coders, &bundle)
}

fn load_error_for_template(
    initial_command: &str,
    directory: &str,
    session_extra: &str,
    bundle_extra: &str,
) -> String {
    let temporary = TempDir::new().expect("temporary");
    let coders = tmux_coders(initial_command, "\"conduct\"");
    let bundle = bundle_document(directory, None, session_extra, bundle_extra);
    let root = write_config(&temporary, "alpha", &coders, &bundle);
    load_bundle_configuration(&root, "alpha")
        .expect_err("invalid template loads")
        .to_string()
}

#[test]
fn accepts_dir_flag_composition() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template("\"cmd --dir={{session-directory}}\"", &directory, "", "");
    assert!(
        command.contains(&format!("--dir='{directory}'")),
        "expected composed dir flag, got: {command}"
    );
}

#[test]
fn accepts_mount_project_directory_form() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd --mount {{project-name}}:{{session-directory}}\"",
        &directory,
        "project-name = \"web\"\n",
        "",
    );
    assert!(
        command.contains(&format!("--mount web:'{directory}'")),
        "expected composed mount word, got: {command}"
    );
}

#[test]
fn rejects_glob_affix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd {{session-directory}}*\"", &directory, "", "");
    assert!(
        error.contains("not shell-literal-safe"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_expansion_affix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd x${IFS}{{session-directory}}\"", &directory, "", "");
    assert!(
        error.contains("not shell-literal-safe"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_operator_affix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd {{session-directory}};next\"", &directory, "", "");
    assert!(
        error.contains("not shell-literal-safe"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_comment_tilde_and_caret_affixes() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    for affixed in [
        "\"cmd #{{session-directory}}\"",
        "\"cmd ~{{session-directory}}\"",
        "\"cmd ^{{session-directory}}\"",
    ] {
        let error = load_error_for_template(affixed, &directory, "", "");
        assert!(
            error.contains("not shell-literal-safe"),
            "unexpected error for {affixed}: {error}"
        );
    }
}

#[test]
fn rejects_assignment_prefix() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd A={{session-directory}}\"", &directory, "", "");
    assert!(
        error.contains("could form a shell assignment"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_variable_composing_assignment_shape() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"cmd {{project-name}}={{session-directory}}\"",
        &directory,
        "project-name = \"ROOT\"\n",
        "",
    );
    assert!(
        error.contains("could form a shell assignment"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_directory_composing_assignment_shape() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd A{{session-directory}}=x\"", &directory, "", "");
    assert!(
        error.contains("could form a shell assignment"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_coder_session_id_adjacency() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let coders = tmux_coders(
        "\"conduct\"",
        "\"conduct {{coder-session-id}}{{session-directory}}\"",
    );
    let bundle = bundle_document(&directory, Some("abc123"), "", "");
    let root = write_config(&temporary, "alpha", &coders, &bundle);
    let error = load_bundle_configuration(&root, "alpha")
        .expect_err("coder-adjacent directory loads")
        .to_string();
    assert!(
        error.contains("must not compose"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_unknown_placeholder_adjacent_to_directory() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error =
        load_error_for_template("\"cmd {{session-directory}}{{bogus}}\"", &directory, "", "");
    assert!(
        error.contains("unknown placeholder '{{bogus}}'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_quote_adjacent_right_of_directory() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd {{session-directory}}'x'\"", &directory, "", "");
    assert!(
        error.contains("not shell-literal-safe"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_immediate_right_escape() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error =
        load_error_for_template("\"cmd {{session-directory}}\\\\ next\"", &directory, "", "");
    assert!(
        error.contains("not shell-literal-safe"),
        "unexpected error: {error}"
    );
}

#[test]
fn accepts_safe_affix_alphabet() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd aZ09-_.:/=,+@%{{session-directory}}\"",
        &directory,
        "",
        "",
    );
    assert!(
        command.contains(&format!("aZ09-_.:/=,+@%'{directory}'")),
        "expected full safe alphabet to compose, got: {command}"
    );
}

const SPECIAL_DIRECTORY_NAME: &str = "two words'o\\clock";

#[cfg(feature = "pty")]
#[test]
fn pty_tokenizer_returns_composed_dir_flag_as_single_argument() {
    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = command_for_template("\"cmd --dir={{session-directory}}\"", &directory, "", "");
    let tokens = tokenize_command(&command).expect("tokenize rendered command");
    assert!(
        tokens
            .iter()
            .any(|token| token == &format!("--dir={directory}")),
        "composed dir flag must tokenize as one argument, got: {tokens:?}"
    );
}

#[cfg(unix)]
#[test]
fn shell_receives_composed_dir_flag_as_single_argument() {
    use std::process::Command;

    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = command_for_template(
        "\"printf '<%s>' --dir={{session-directory}}\"",
        &directory,
        "",
        "",
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .expect("run shell seam");
    assert!(output.status.success(), "shell failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("<--dir={directory}>")
    );
}

#[cfg(feature = "pty")]
#[test]
fn pty_tokenizer_returns_mount_form_as_single_argument() {
    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = command_for_template(
        "\"cmd --mount {{project-name}}:{{session-directory}}\"",
        &directory,
        "project-name = \"web\"\n",
        "",
    );
    let tokens = tokenize_command(&command).expect("tokenize rendered command");
    assert!(
        tokens
            .iter()
            .any(|token| token == &format!("web:{directory}")),
        "composed mount word must tokenize as one argument, got: {tokens:?}"
    );
}

#[cfg(unix)]
#[test]
fn shell_receives_mount_form_as_single_argument() {
    use std::process::Command;

    let temporary = TempDir::new().expect("temporary");
    let special = temporary.path().join(SPECIAL_DIRECTORY_NAME);
    fs::create_dir(&special).expect("create special directory");
    let directory = special.display().to_string();
    let command = command_for_template(
        "\"printf '<%s>' {{project-name}}:{{session-directory}}\"",
        &directory,
        "project-name = \"web\"\n",
        "",
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .expect("run shell seam");
    assert!(output.status.success(), "shell failed: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("<web:{directory}>")
    );
}

#[test]
fn resolves_project_name_from_session_override() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "project-name = \"web\"\n",
        "project-name-from = \"bundle-name\"\n",
    );
    assert!(
        command.contains("cmd web"),
        "session override must win, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_session_override_above_bundle_rule() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let bundle = bundle_document(
        &directory,
        None,
        "project-name = \"web\"\n",
        "project-name-from = \"bundle-name\"\n",
    );
    let coders = tmux_coders("\"cmd {{project-name}}\"", "\"conduct\"");
    let command = start_command("alpha", &coders, &bundle);
    assert!(
        command.contains("cmd web"),
        "override above bundle rule must win without ambiguity, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_session_rule_above_bundle_rule() {
    let temporary = TempDir::new().expect("temporary");
    let named = temporary.path().join("checkout");
    fs::create_dir(&named).expect("create named directory");
    let directory = named.display().to_string();
    let command = command_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "project-name-from = \"session-directory-basename\"\n",
        "project-name-from = \"bundle-name\"\n",
    );
    assert!(
        command.contains("cmd checkout"),
        "session rule must beat bundle rule, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_bundle_rule() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "",
        "project-name-from = \"bundle-name\"\n",
    );
    assert!(
        command.contains("cmd alpha"),
        "bundle rule must yield the bundle id, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_directory_basename_by_default() {
    let temporary = TempDir::new().expect("temporary");
    let named = temporary.path().join("checkout");
    fs::create_dir(&named).expect("create named directory");
    let directory = named.display().to_string();
    let command = command_for_template("\"cmd {{project-name}}\"", &directory, "", "");
    assert!(
        command.contains("cmd checkout"),
        "default must yield the directory basename, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_dotted_bundle_name() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let coders = tmux_coders("\"cmd {{project-name}}\"", "\"conduct\"");
    let bundle = bundle_document(
        &directory,
        None,
        "",
        "project-name-from = \"bundle-name\"\n",
    );
    let command = start_command("team.one", &coders, &bundle);
    assert!(
        command.contains("cmd team.one"),
        "dotted bundle id must resolve, got: {command}"
    );
}

#[test]
fn resolves_project_name_from_long_bundle_name() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let long = "b".repeat(120);
    let coders = tmux_coders("\"cmd {{project-name}}\"", "\"conduct\"");
    let bundle = bundle_document(
        &directory,
        None,
        "",
        "project-name-from = \"bundle-name\"\n",
    );
    let command = start_command(&long, &coders, &bundle);
    assert!(
        command.contains(&format!("cmd {long}")),
        "long bundle id must resolve, got: {command}"
    );
}

#[test]
fn substitutes_project_name_raw() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd pre-{{project-name}}.post\"",
        &directory,
        "project-name = \"web-2.0_x\"\n",
        "",
    );
    assert!(
        command.contains("cmd pre-web-2.0_x.post"),
        "project name must substitute raw, got: {command}"
    );
}

#[test]
fn rejects_session_with_both_project_name_and_rule() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"conduct\"",
        &directory,
        "project-name = \"web\"\nproject-name-from = \"bundle-name\"\n",
        "",
    );
    assert!(
        error.contains("both project-name and project-name-from"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_unknown_bundle_project_name_from_rule() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"conduct\"",
        &directory,
        "",
        "project-name-from = \"git-remote\"\n",
    );
    assert!(
        error.contains("must be 'session-directory-basename' or 'bundle-name'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_unknown_session_project_name_from_rule() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"conduct\"",
        &directory,
        "project-name-from = \"git-remote\"\n",
        "",
    );
    assert!(
        error.contains("must be 'session-directory-basename' or 'bundle-name'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_non_conforming_explicit_project_name() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"conduct\"",
        &directory,
        "project-name = \"my project\"\n",
        "",
    );
    assert!(
        error.contains("must be a portable path segment"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_used_non_conforming_basename() {
    let temporary = TempDir::new().expect("temporary");
    let spaced = temporary.path().join("my project");
    fs::create_dir(&spaced).expect("create spaced directory");
    let directory = spaced.display().to_string();
    let error = load_error_for_template("\"cmd {{project-name}}\"", &directory, "", "");
    assert!(
        error.contains("must be a portable path segment"),
        "unexpected error: {error}"
    );
}

#[test]
fn loads_non_conforming_basename_without_project_name_usage() {
    let temporary = TempDir::new().expect("temporary");
    let spaced = temporary.path().join("my project");
    fs::create_dir(&spaced).expect("create spaced directory");
    let directory = spaced.display().to_string();
    let command = command_for_template("\"cmd {{session-directory}}\"", &directory, "", "");
    assert!(
        command.contains(&format!("'{directory}'")),
        "session without project-name usage must load, got: {command}"
    );
}

#[test]
fn rejects_unknown_project_name_misspelling() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd {{project-nam}}\"", &directory, "", "");
    assert!(
        error.contains("unknown placeholder '{{project-nam}}'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_blank_explicit_project_name() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "project-name = \" \"\n",
        "",
    );
    assert!(
        error.contains("must be a portable path segment"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_blank_bundle_project_name_from() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "",
        "project-name-from = \" \"\n",
    );
    assert!(
        error.contains("must be 'session-directory-basename' or 'bundle-name'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_blank_session_project_name_from() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "project-name-from = \" \"\n",
        "",
    );
    assert!(
        error.contains("must be 'session-directory-basename' or 'bundle-name'"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_blank_project_name_with_rule_as_ambiguous() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template(
        "\"cmd {{project-name}}\"",
        &directory,
        "project-name = \" \"\nproject-name-from = \"bundle-name\"\n",
        "",
    );
    assert!(
        error.contains("both project-name and project-name-from"),
        "unexpected error: {error}"
    );
}

#[test]
fn substitutes_bundle_name_raw() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template("\"cmd --bundle {{bundle-name}}\"", &directory, "", "");
    assert!(
        command.contains("cmd --bundle alpha"),
        "bundle name must substitute raw, got: {command}"
    );
}

#[test]
fn bundle_name_stays_valid_in_quotes() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template("\"cmd '{{bundle-name}}'\"", &directory, "", "");
    assert!(
        command.contains("cmd 'alpha'"),
        "bundle name must stay valid in any context, got: {command}"
    );
}

#[test]
fn resolves_bundle_name_from_dotted_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let coders = tmux_coders("\"cmd {{bundle-name}}\"", "\"conduct\"");
    let bundle = bundle_document(&directory, None, "", "");
    let command = start_command("team.one", &coders, &bundle);
    assert!(
        command.contains("cmd team.one"),
        "dotted bundle id must resolve, got: {command}"
    );
}

#[test]
fn accepts_bundle_name_adjacent_to_directory() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let command = command_for_template(
        "\"cmd {{bundle-name}}{{session-directory}}\"",
        &directory,
        "",
        "",
    );
    assert!(
        command.contains(&format!("alpha'{directory}'")),
        "grammar-safe bundle name must compose, got: {command}"
    );
}

#[test]
fn rejects_bundle_filenames_with_shell_unsafe_characters() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    for bundle_name in [
        "alpha;printf-pwned",
        "alpha pwned",
        "alpha'pwned",
        "alpha$pwned",
        "alpha*pwned",
        "../pwned",
    ] {
        let coders = tmux_coders("\"cmd {{bundle-name}}\"", "\"conduct\"");
        let bundle = bundle_document(&directory, None, "", "");
        let root = write_config(&temporary, bundle_name, &coders, &bundle);
        let error = load_bundle_configuration(&root, bundle_name)
            .expect_err("unsafe bundle name loads")
            .to_string();
        assert!(
            error.contains("must be a portable path segment"),
            "unexpected error for {bundle_name}: {error}"
        );
    }
}

#[test]
fn rejects_unknown_bundle_name_misspelling() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let error = load_error_for_template("\"cmd {{bundle-nam}}\"", &directory, "", "");
    assert!(
        error.contains("unknown placeholder '{{bundle-nam}}'"),
        "unexpected error: {error}"
    );
}

#[test]
fn placeholder_shaped_directory_value_stays_literal() {
    let temporary = TempDir::new().expect("temporary");
    let shaped = temporary.path().join("w{{project-name}}");
    fs::create_dir(&shaped).expect("create brace directory");
    let directory = shaped.display().to_string();
    let command = command_for_template(
        "\"cmd {{session-directory}} {{project-name}}\"",
        &directory,
        "project-name = \"web\"\n",
        "",
    );
    assert_eq!(
        command.as_str(),
        &format!("cmd '{directory}' web"),
        "substituted directory bytes must never rescan, got: {command}"
    );
}

#[test]
fn placeholder_shaped_coder_session_id_stays_literal() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().display().to_string();
    let coders = tmux_coders(
        "\"conduct\"",
        "\"cmd {{coder-session-id}} {{bundle-session-id}}\"",
    );
    let bundle = bundle_document(&directory, Some("a{{bundle-session-id}}b"), "", "");
    let root = write_config(&temporary, "alpha", &coders, &bundle);
    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let TargetConfiguration::Tmux(target) = &loaded.members[0].target else {
        panic!("expected tmux target");
    };
    assert_eq!(
        target.start_command.as_str(),
        "cmd a{{bundle-session-id}}b session-a",
        "substituted coder id bytes must never rescan, got: {}",
        target.start_command
    );
}
