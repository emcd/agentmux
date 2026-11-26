//! Shared fixtures for direct public-consumer operation tests.
//!
//! Every operation test drives the public runtime and operations APIs
//! only: no MCP server, no stdio transport, no rmcp router, no agent
//! harness, no MCP association. The fixtures below build an
//! in-process runtime with two bundles (party: alpha, bravo; other:
//! carol), provisioned principals, and verified contexts; per-operation
//! tests add only their operation construction on top.

use std::sync::Arc;

use agentmux::relay::{
    CredentialSource, DeliveryConfiguration, IdentityDescriptor,
    ProvisionParams, RelayRuntime, VerifiedPrincipal, provision_principal,
    verify_identity,
};
use tempfile::TempDir;

pub(super) struct OperationFixture {
    pub(super) _temporary: TempDir,
    pub(super) runtime: Arc<RelayRuntime>,
}

/// Variant whose sessions use the given coder: `"shell"` backs tmux
/// sessions, `"acp"` backs workerless-snapshot targets that need no
/// live session. The coder definition matches the reference.
fn write_bundles_with_coder(
    temporary: &TempDir,
    send_scope: &str,
    coder: &str,
    look_scope: &str,
) {
    let configuration_root = temporary.path().join("conf");
    let bundles = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("policies.toml"),
        format!(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "{look_scope}"
send = "{send_scope}"
"#
        ),
    )
    .expect("write policies file");
    for (bundle, members) in
        [("party", vec!["alpha", "bravo"]), ("other", vec!["carol"])]
    {
        let sessions: Vec<String> = members
            .iter()
            .map(|id| {
                format!(
                    r#"
[[sessions]]
id = "{id}"
name = "{id}"
directory = "/tmp"
coder = "{coder}"
"#
                )
            })
            .collect();
        std::fs::write(
            bundles.join(format!("{bundle}.toml")),
            format!(
                "format-version = 1\nautostart = true\n{}",
                sessions.join("")
            ),
        )
        .expect("write bundle configuration");
    }
    std::fs::write(
        configuration_root.join("coders.toml"),
        if coder == "acp" {
            r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "sh -lc 'cat >/dev/null'"
"#
            .to_string()
        } else {
            r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#
            .to_string()
        },
    )
    .expect("write coders file");
}

pub(super) fn operation_fixture(send_scope: &str) -> OperationFixture {
    operation_fixture_with_coder(send_scope, "shell")
}

/// Fixture variant whose sessions use the given coder: `"shell"`
/// backs tmux sessions, `"acp"` backs workerless-snapshot targets
/// that need no live session.
pub(super) fn operation_fixture_with_coder(
    send_scope: &str,
    coder: &str,
) -> OperationFixture {
    operation_fixture_full(send_scope, coder, "self")
}

/// Fixture variant with an explicit look scope: cross-bundle
/// inspection needs `all`, while the default `self` keeps the
/// denial tests meaningful.
pub(super) fn operation_fixture_with_look(
    send_scope: &str,
    coder: &str,
    look_scope: &str,
) -> OperationFixture {
    operation_fixture_full(send_scope, coder, look_scope)
}

fn operation_fixture_full(
    send_scope: &str,
    coder: &str,
    look_scope: &str,
) -> OperationFixture {
    let temporary = TempDir::new().expect("temporary directory");
    write_bundles_with_coder(&temporary, send_scope, coder, look_scope);
    let runtime = Arc::new(
        RelayRuntime::init(
            temporary.path().join("conf"),
            temporary.path().join("state"),
            DeliveryConfiguration::default(),
        )
        .expect("runtime init"),
    );
    OperationFixture {
        _temporary: temporary,
        runtime,
    }
}

pub(super) fn verified_context(
    runtime: &RelayRuntime,
    principal_id: &str,
) -> VerifiedPrincipal {
    let psk = provision_principal(
        runtime,
        ProvisionParams::new(principal_id.to_string()),
    )
    .expect("provision principal")
    .psk
    .expect("one-time raw PSK");
    verify_identity(
        runtime,
        &IdentityDescriptor::new(
            principal_id.to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("verify identity")
}

/// A relay-wide operator binding, so a GLOBAL principal has resolved
/// controls: GLOBAL homes have no bundle members, so their policy
/// comes from the TUI session presets, independent of any bundle.
pub(super) fn authorize_global_operator(
    fixture: &OperationFixture,
    operator_id: &str,
) {
    let configuration_root = fixture._temporary.path().join("conf");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "all"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "all"
send = "all"
"#,
    )
    .expect("write operator policies");
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            r#"
default-session = "{operator_id}"

[[sessions]]
id = "{operator_id}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write operator binding");
}
