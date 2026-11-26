//! Shared fixtures for the public-dispatch integration suites.
//!
//! Named for the first consumer (`dispatch_send`); shared by every
//! public-handler suite so bundle, policy, and identity setup stays
//! identical across send/look/raww/list coverage.

use agentmux::relay::{
    CredentialSource, DeliveryConfiguration, IdentityDescriptor,
    ProvisionParams, RelayRuntime, provision_principal, verify_identity,
};
use tempfile::TempDir;

/// Writes the shared bundle/policy/coder layout, with every session
/// referencing the tmux shell coder.
pub(super) fn write_bundles(
    temporary: &TempDir,
    send_scope: &str,
    extra_other_members: &[(&str, &str)],
) {
    write_bundles_with_coder(
        temporary,
        send_scope,
        extra_other_members,
        "shell",
    );
}

/// Variant of [`write_bundles`] whose sessions reference `coder`
/// (`"shell"` for tmux, `"acp"` for workerless-snapshot ACP targets).
/// The coder definition block matches the reference.
pub(super) fn write_bundles_with_coder(
    temporary: &TempDir,
    send_scope: &str,
    extra_other_members: &[(&str, &str)],
    coder: &str,
) {
    let configuration_root = temporary.path().join("conf");
    let bundles = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
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
    let mut other_members = vec![("carol", "Carol")];
    other_members.extend_from_slice(extra_other_members);
    for (bundle, members) in [
        ("party", vec![("alpha", "Alpha"), ("bravo", "Bravo")]),
        ("other", other_members),
    ] {
        let sessions: Vec<String> = members
            .iter()
            .map(|(id, name)| {
                format!(
                    r#"
[[sessions]]
id = "{id}"
name = "{name}"
directory = "/tmp"
coder = "{coder}"
"#
                )
            })
            .collect();
        std::fs::write(
            bundles.join(format!("{bundle}.toml")),
            // Hosted bundles declare autostart explicitly: an absent key
            // means Hold, so fixtures model running bundles honestly.
            format!(
                "format-version = 1\nautostart = true\n{}",
                sessions.join("")
            ),
        )
        .expect("write bundle configuration");
    }
}

pub(super) struct SendFixture {
    pub(super) _temporary: TempDir,
    pub(super) runtime: RelayRuntime,
}

pub(super) fn send_fixture(send_scope: &str) -> SendFixture {
    send_fixture_with_members(send_scope, &[])
}

pub(super) fn send_fixture_with_coder(
    send_scope: &str,
    coder: &str,
) -> SendFixture {
    let temporary = TempDir::new().expect("temporary directory");
    write_bundles_with_coder(&temporary, send_scope, &[], coder);
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    let runtime = RelayRuntime::init(
        configuration_root,
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("runtime init");
    SendFixture {
        _temporary: temporary,
        runtime,
    }
}

pub(super) fn send_fixture_with_members(
    send_scope: &str,
    extra_other_members: &[(&str, &str)],
) -> SendFixture {
    let temporary = TempDir::new().expect("temporary directory");
    write_bundles(&temporary, send_scope, extra_other_members);
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    let runtime = RelayRuntime::init(
        configuration_root,
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("runtime init");
    SendFixture {
        _temporary: temporary,
        runtime,
    }
}

/// Verifies `principal_id` with the given one-time PSK, returning the
/// live context dispatch requires.
pub(super) fn verified_context(
    runtime: &RelayRuntime,
    principal_id: &str,
    psk: &str,
) -> agentmux::relay::VerifiedPrincipal {
    verify_identity(
        runtime,
        &IdentityDescriptor::new(
            principal_id.to_string(),
            CredentialSource::DirectPsk(psk.to_string()),
        ),
    )
    .expect("verify identity")
}

pub(super) fn provisioned_psk(
    runtime: &RelayRuntime,
    principal_id: &str,
) -> String {
    provision_principal(
        runtime,
        ProvisionParams::new(principal_id.to_string()),
    )
    .expect("provision principal")
    .psk
    .expect("one-time raw PSK")
}
