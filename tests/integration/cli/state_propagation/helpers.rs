use std::{fs, path::Path};

/// A state root the member declares for itself, which the relay must overwrite.
pub(super) const MEMBER_DECLARED_STATE_ROOT: &str = "/nowhere/member-declared";

/// A blank declaration, which must be overwritten rather than suppressing the
/// stamp. Blank reads as absent everywhere else, so this is the case where an
/// upsert-if-absent implementation would look correct and still break the
/// rendezvous.
pub(super) const MEMBER_BLANK_STATE_ROOT: &str = "";

pub(super) fn declare_member_state_directory(config_root: &Path, bundle_name: &str, value: &str) {
    let path = config_root
        .join("bundles")
        .join(format!("{bundle_name}.toml"));
    let mut bundle = fs::read_to_string(&path).expect("read bundle configuration");
    bundle.push_str(&format!(
        "\n[[sessions.environment]]\nname = \"AGENTMUX_STATE_DIRECTORY\"\nvalue = \"{value}\"\n"
    ));
    fs::write(&path, bundle).expect("write bundle configuration");
}

pub(super) fn recorded_new_session(log: &str) -> &str {
    let mut lines = log
        .lines()
        .filter(|line| line.contains("new-session"))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one new-session invocation, got:\n{log}"
    );
    lines.pop().expect("one new-session line")
}
