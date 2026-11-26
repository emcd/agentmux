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

fn write_bundles(temporary: &TempDir, send_scope: &str) {
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
look = "self"
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
coder = "shell"
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
}

pub(super) fn operation_fixture(send_scope: &str) -> OperationFixture {
    let temporary = TempDir::new().expect("temporary directory");
    write_bundles(&temporary, send_scope);
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
look = "home"
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
