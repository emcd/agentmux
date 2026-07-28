//! Per-operation integration tests for the `relay` operation surface:
//! - [`list`]: list operation tests (configured sessions, transport
//!   reporting, policy gates).
//! - [`send`]: send operation tests (target resolution, broadcast,
//!   async dispatch).
//! - [`look`]: look operation tests (capability gates, snapshot
//!   construction, rejection matrix).
//!
//! Shared helpers (per-test fixture writers, the `dispatch_request`
//! adapter that maps the test surface onto the library entry point) live
//! in this hub. The `relay::error`, `relay::peer_connection`,
//! `relay::preflight`, `relay::principal_store`, `relay::raww`,
//! `relay::relay_configuration`, `relay::request_validation`, and
//! `relay::startup_failures` clusters are unchanged — they were already
//! organized as per-concern sibling files in the original
//! `tests/unit/relay/` directory and are sibling modules under this
//! hub.

use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::{RelayRequest, RelayResponse, handle_request};
use tempfile::TempDir;

mod contract;
mod error;
mod list;
mod look;
mod peer_connection;
mod preflight;
mod principal_store;
mod raww;
mod relay_configuration;
mod request_validation;
mod send;
mod startup_failures;

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    tmux_socket: &std::path::Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    let runtime_directory = tmux_socket
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
}

fn write_tui_configuration(roots: &ConfigurationRoots, policy: &str) {
    std::fs::write(
        roots.base_layer().join("users.toml"),
        format!(
            r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "{policy}"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

fn write_tui_configuration_with_session_id(
    roots: &ConfigurationRoots,
    policy: &str,
    session_id: &str,
) {
    std::fs::write(
        roots.base_layer().join("users.toml"),
        format!(
            r#"
default-session = "{session_id}"

[[sessions]]
id = "{session_id}"
policy = "{policy}"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

fn write_bundle(temporary: &TempDir, name: &str) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
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
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"

# A privileged cross-namespace operator: a relay-wide principal's home is the
# GLOBAL namespace, so reaching into a bundle requires all.
[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    ConfigurationRoots::single(root)
}

fn write_single_member_bundle(temporary: &TempDir, name: &str) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
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
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    ConfigurationRoots::single(root)
}

fn write_acp_bundle(temporary: &TempDir, name: &str) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "sh -lc 'cat >/dev/null'"
"#,
    )
    .expect("write coders file");
    std::fs::write(
        root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"

# Privileged cross-namespace operator (see write_bundle).
[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies file");
    let body = r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "acp"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "acp"
"#;
    std::fs::write(bundles.join(format!("{name}.toml")), body).expect("write bundle file");
    ConfigurationRoots::single(root)
}

fn write_bundle_with_policy(
    temporary: &TempDir,
    name: &str,
    bundle_body: &str,
    policy_body: Option<&str>,
) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
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
    .expect("write coders file");
    if let Some(policy_body) = policy_body {
        std::fs::write(root.join("policies.toml"), policy_body).expect("write policies file");
    }
    std::fs::write(bundles.join(format!("{name}.toml")), bundle_body).expect("write bundle file");
    ConfigurationRoots::single(root)
}
