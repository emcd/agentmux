use agentmux::configuration::ConfigurationRoots;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(crate) fn write_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    sessions: &[&str],
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let session_specs = sessions
        .iter()
        .map(|session| SessionSpec {
            id: (*session).to_string(),
            name: Some((*session).to_string()),
            directory: PathBuf::from("/tmp"),
            coder: "default".to_string(),
            coder_session_id: None,
        })
        .collect::<Vec<_>>();
    write_bundle_configuration_members(
        root,
        bundle_name,
        &coders,
        &session_specs,
    )
}

#[derive(Clone)]
pub(crate) struct CoderSpec {
    pub(crate) id: String,
    pub(crate) initial_command: String,
    pub(crate) resume_command: String,
    pub(crate) prompt_regex: Option<String>,
    pub(crate) prompt_inspect_lines: Option<usize>,
    pub(crate) prompt_idle_column: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct SessionSpec {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) directory: PathBuf,
    pub(crate) coder: String,
    pub(crate) coder_session_id: Option<String>,
}

pub(crate) fn write_bundle_configuration_members(
    root: &Path,
    bundle_name: &str,
    coders: &[CoderSpec],
    sessions: &[SessionSpec],
) -> ConfigurationRoots {
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    let mut coders_toml = String::from("format-version = 1\n");
    for coder in coders {
        coders_toml.push_str(
            format!(
                "\n[[coders]]\nid = \"{}\"\n[coders.tmux]\ninitial-command = \"{}\"\nresume-command = \"{}\"\n",
                coder.id, coder.initial_command, coder.resume_command
            )
            .as_str(),
        );
        if let Some(prompt_regex) = coder.prompt_regex.as_deref() {
            coders_toml.push_str(
                format!("prompt-regex = \"{}\"\n", prompt_regex).as_str(),
            );
        }
        if let Some(lines) = coder.prompt_inspect_lines {
            coders_toml.push_str(
                format!("prompt-inspect-lines = {lines}\n").as_str(),
            );
        }
        if let Some(column) = coder.prompt_idle_column {
            coders_toml
                .push_str(format!("prompt-idle-column = {column}\n").as_str());
        }
    }
    fs::write(config_root.join("coders.toml"), coders_toml)
        .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
raww = "home"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(config_root.join("users.toml"), "").expect("write users config");

    let mut bundle_toml =
        String::from("format-version = 1\nautostart = true\n");
    for session in sessions {
        bundle_toml.push_str(
            format!("\n[[sessions]]\nid = \"{}\"\n", session.id).as_str(),
        );
        if let Some(name) = session.name.as_deref() {
            bundle_toml.push_str(format!("name = \"{}\"\n", name).as_str());
        }
        bundle_toml.push_str(
            format!(
                "directory = \"{}\"\ncoder = \"{}\"\n",
                session.directory.display(),
                session.coder
            )
            .as_str(),
        );
        if let Some(coder_session_id) = session.coder_session_id.as_deref() {
            bundle_toml.push_str(
                format!("coder-session-id = \"{}\"\n", coder_session_id)
                    .as_str(),
            );
        }
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    ConfigurationRoots::single(config_root)
}

/// Writes a bundle with one tmux coder session and one bare `pubsub`-marker
/// session, reusing the standard coders/policies/users config. Used to prove a
/// configured Pubsub member is constructed as the forward-declared stub transport
/// and yields a not-implemented outcome rather than being misrouted to tmux.
pub(crate) fn write_bundle_with_pubsub_member(
    root: &Path,
    bundle_name: &str,
    tmux_session: &str,
    pubsub_session: &str,
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let sessions = vec![SessionSpec {
        id: tmux_session.to_string(),
        name: Some(tmux_session.to_string()),
        directory: PathBuf::from("/tmp"),
        coder: "default".to_string(),
        coder_session_id: None,
    }];
    // Reuse the helper to lay down coders/policies/users; then re-write the bundle
    // file to add the bare pubsub session alongside the tmux one.
    let config_root = write_bundle_configuration_members(
        root,
        bundle_name,
        &coders,
        &sessions,
    );
    let mut bundle_toml =
        String::from("format-version = 1\nautostart = true\n");
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{tmux_session}\"\nname = \"{tmux_session}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
        )
        .as_str(),
    );
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{pubsub_session}\"\ndirectory = \"/tmp\"\n[sessions.pubsub]\n",
        )
        .as_str(),
    );
    fs::write(
        config_root
            .base_layer()
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle_toml,
    )
    .expect("write bundle config with pubsub member");
    config_root
}

/// Writes a single-session tmux bundle whose bundle file declares the given
/// top-level `environment` entries, reusing the standard coders/policies/users
/// config. Used to prove the merged environment reaches the tmux
/// session-creation call as `new-session -e KEY=VALUE` flags.
pub(crate) fn write_bundle_configuration_with_environment(
    root: &Path,
    bundle_name: &str,
    session: &str,
    environment: &[(&str, &str)],
) -> ConfigurationRoots {
    let coders = vec![CoderSpec {
        id: "default".to_string(),
        initial_command: "sh -lc 'exec sleep 45'".to_string(),
        resume_command: "sh -lc 'exec sleep 45'".to_string(),
        prompt_regex: None,
        prompt_inspect_lines: None,
        prompt_idle_column: None,
    }];
    let sessions = vec![SessionSpec {
        id: session.to_string(),
        name: Some(session.to_string()),
        directory: PathBuf::from("/tmp"),
        coder: "default".to_string(),
        coder_session_id: None,
    }];
    let config_root = write_bundle_configuration_members(
        root,
        bundle_name,
        &coders,
        &sessions,
    );

    let mut bundle_toml =
        String::from("format-version = 1\nautostart = true\n");
    for (name, value) in environment {
        bundle_toml.push_str(
            format!(
                "\n[[environment]]\nname = \"{name}\"\nvalue = \"{value}\"\n"
            )
            .as_str(),
        );
    }
    bundle_toml.push_str(
        format!(
            "\n[[sessions]]\nid = \"{session}\"\nname = \"{session}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
        )
        .as_str(),
    );
    fs::write(
        config_root
            .base_layer()
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        bundle_toml,
    )
    .expect("write bundle config with environment");
    config_root
}

/// Writes a bundle whose sole coder is a stdio ACP coder backed by a stub that
/// answers `initialize`/`session/new` but deliberately never completes a
/// `session/prompt` turn, so a send to an ACP session leaves a turn in flight.
/// The stub appends every spawned child's PID to `pid_file` and logs each
/// received JSON-RPC line to `log_file`. Stub configuration rides in through the
/// coder-level `[[coders.environment]]` surface (the same channel the ACP
/// integration suite uses), which the relay applies to the spawned child.
///
/// Used to reproduce the in-flight-ACP-turn shutdown condition for
/// `issues/relay/49`: relay shutdown must reap the delivery thread's bounded
/// prompt-completion wait within a bounded window rather than pinning the
/// process until SIGKILL.
pub(crate) fn write_acp_hang_bundle_configuration(
    root: &Path,
    bundle_name: &str,
    sessions: &[&str],
    stub_path: &Path,
    log_file: &Path,
    pid_file: &Path,
) -> ConfigurationRoots {
    write_acp_hang_stub(stub_path);
    let config_root = root.join("config");
    let bundles = config_root.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");

    let coders = format!(
        r#"format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "{command}"

[[coders.environment]]
name = "ACP_LOG_FILE"
value = "{log}"

[[coders.environment]]
name = "ACP_PID_FILE"
value = "{pid}"
"#,
        command = stub_path.display(),
        log = log_file.display(),
        pid = pid_file.display(),
    );
    fs::write(config_root.join("coders.toml"), coders)
        .expect("write coders config");

    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
raww = "home"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(config_root.join("users.toml"), "").expect("write users config");

    let mut bundle_toml =
        String::from("format-version = 1\nautostart = true\n");
    for session in sessions {
        bundle_toml.push_str(
            format!(
                "\n[[sessions]]\nid = \"{session}\"\nname = \"{session}\"\ndirectory = \"/tmp\"\ncoder = \"acp\"\n",
            )
            .as_str(),
        );
    }
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml)
        .expect("write bundle config");
    ConfigurationRoots::single(config_root)
}

/// Writes a minimal stdio ACP agent stub that hangs on `session/prompt`.
///
/// It answers `initialize`, `session/new`, and `session/load` so worker
/// bootstrap succeeds and the worker reaches `Available`, but on
/// `session/prompt` it emits no `stopReason` result and loops back to read.
/// The turn therefore stays in flight and the child stays alive (blocked on
/// read) until relay shutdown kills it or closes its stdin (EOF ends the loop).
fn write_acp_hang_stub(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

log_file="${ACP_LOG_FILE:?}"
pid_file="${ACP_PID_FILE:?}"
printf '%s\n' "$$" >> "$pid_file"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log_file"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptSession":true}}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sess-hang"}}\n' "$id"
      ;;
    *'"method":"session/load"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      : # never respond -- leave the turn in flight
      ;;
  esac
done
"#;
    fs::write(path, script).expect("write ACP hang stub");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("set ACP hang stub executable");
}
