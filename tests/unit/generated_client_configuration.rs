//! Guards the prohibition on `--state-directory` in generated coder client
//! configuration.
//!
//! A coder template emits an `agentmux host mcp` command line into a committed
//! file, so a flag placed there is committed content wearing CLI intent. This
//! one would outrank the `AGENTMUX_STATE_DIRECTORY` the relay injects at spawn
//! and silently put the child on a different relay than the one that spawned
//! it — the rendezvous failure the injection exists to prevent, reintroduced by
//! a file nobody edits by hand.
//!
//! These artifacts are generated from a Copier template upstream, so this test
//! cannot stop the constraint being violated at the source. It is here because
//! this repository is the only place the constraint can be checked
//! mechanically, and a regeneration that reintroduces the flag fails here.

use std::{fs, path::PathBuf};

/// In-repo artifacts carrying an `agentmux host mcp` command line.
///
/// `coders/claude/settings.json` is deliberately absent: it carries
/// environment, tool permissions, and sandbox settings, and names agentmux only
/// as tool identifiers. It emits no command line, so it has nothing to guard.
const COMMAND_LINE_ARTIFACTS: [&str; 3] = [
    ".auxiliary/configuration/coders/opencode/settings.jsonc",
    ".auxiliary/configuration/coders/codex/config.toml",
    ".auxiliary/configuration/mcp-servers.json",
];

#[test]
fn generated_client_configuration_emits_no_state_directory_flag() {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for relative in COMMAND_LINE_ARTIFACTS {
        let path = repository_root.join(relative);
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|source| panic!("read {}: {source}", path.display()));
        assert!(
            !contents.contains("--state-directory"),
            "{relative} must not emit --state-directory: a committed flag outranks the \
             AGENTMUX_STATE_DIRECTORY the relay injects at spawn, sending the child to a \
             relay that did not spawn it"
        );
    }
}

#[test]
fn every_guarded_artifact_still_carries_an_agentmux_command_line() {
    // Without this the guard above passes vacuously once a template stops
    // emitting the command line, or renames the file out from under the list.
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for relative in COMMAND_LINE_ARTIFACTS {
        let path = repository_root.join(relative);
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|source| panic!("read {}: {source}", path.display()));
        let normalized = contents.replace(['"', ',', '\n', ' '], "");
        assert!(
            normalized.contains("hostmcp"),
            "{relative} no longer carries an agentmux host mcp command line, so the \
             --state-directory guard covers nothing"
        );
    }
}
