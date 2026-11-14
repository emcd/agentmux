//! In-process send dispatch through the public handler contract.
//!
//! Matrix coverage lives in [`fusion`]; final-admission interleavings live
//! in [`admission`]. Shared fixtures and verified-context helpers live here.

use std::sync::Arc;

use agentmux::relay::{
    BundleCatalog, CredentialSource, DeliveryConfiguration,
    IdentityDescriptor, PeerConnectionManager, ProvisionParams, RelayResponse,
    RelayRuntime, VerifiedPrincipal, provision_principal, verify_identity,
};
use agentmux::runtime::paths::BundleRuntimePaths;
use tempfile::TempDir;

mod admission;
mod fusion;
mod store_hold;
fn write_bundles(
    temporary: &TempDir,
    send_scope: &str,
    extra_other_members: &[(&str, &str)],
) {
    let configuration_root = temporary.path().join("conf");
    let bundles = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
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
coder = "shell"
"#
                )
            })
            .collect();
        std::fs::write(
            bundles.join(format!("{bundle}.toml")),
            format!("format-version = 1\n{}", sessions.join("")),
        )
        .expect("write bundle configuration");
    }
}

struct SendFixture {
    _temporary: TempDir,
    runtime: RelayRuntime,
    catalog: BundleCatalog,
    peer_manager: Arc<PeerConnectionManager>,
}

fn send_fixture(send_scope: &str) -> SendFixture {
    send_fixture_with_members(send_scope, &[])
}

fn send_fixture_with_members(
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
    let catalog =
        BundleCatalog::from_paths(["party", "other"].iter().map(|bundle| {
            BundleRuntimePaths::resolve(&state_root, bundle)
                .expect("bundle paths")
        }));
    let peer_manager =
        Arc::new(PeerConnectionManager::from_configuration(&state_root, &[]));
    SendFixture {
        _temporary: temporary,
        runtime,
        catalog,
        peer_manager,
    }
}

/// Verifies `principal_id` with the given one-time PSK, returning the
/// live context dispatch requires.
fn verified_context(
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

fn provisioned_psk(runtime: &RelayRuntime, principal_id: &str) -> String {
    provision_principal(
        runtime,
        ProvisionParams::new(principal_id.to_string()),
    )
    .expect("provision principal")
    .psk
    .expect("one-time raw PSK")
}

fn send_targets(response: RelayResponse) -> Vec<String> {
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected a send response, got {response:?}");
    };
    let mut targets: Vec<String> = results
        .iter()
        .map(|result| result.target_session.clone())
        .collect();
    targets.sort();
    targets
}

struct ParkFixture {
    _temporary: TempDir,
    runtime: std::sync::Arc<RelayRuntime>,
    catalog: BundleCatalog,
    peer_manager: Arc<PeerConnectionManager>,
    party: String,
    _other: String,
}

fn park_fixture(send_scope: &str, tag: &str) -> ParkFixture {
    let temporary = TempDir::new().expect("temporary directory");
    let party = format!("{tag}party");
    let other = format!("{tag}other");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(configuration_root.join("bundles"))
        .expect("create bundles directory");
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
    for (bundle, members) in [
        (party.as_str(), vec!["alpha", "bravo"]),
        (other.as_str(), vec!["carol"]),
    ] {
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
            configuration_root
                .join("bundles")
                .join(format!("{bundle}.toml")),
            format!("format-version = 1\n{}", sessions.join("")),
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
    let runtime = std::sync::Arc::new(
        RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            DeliveryConfiguration::default(),
        )
        .expect("runtime init"),
    );
    let catalog =
        BundleCatalog::from_paths([&party, &other].iter().map(|bundle| {
            BundleRuntimePaths::resolve(&state_root, bundle)
                .expect("bundle paths")
        }));
    let peer_manager =
        Arc::new(PeerConnectionManager::from_configuration(&state_root, &[]));
    ParkFixture {
        _temporary: temporary,
        runtime,
        catalog,
        peer_manager,
        party,
        _other: other,
    }
}

fn park_provisioned(fixture: &ParkFixture) -> VerifiedPrincipal {
    let sender = format!("alpha@{}", fixture.party);
    let psk = provisioned_psk(&fixture.runtime, &sender);
    verified_context(&fixture.runtime, &sender, &psk)
}

struct AdmitGuard;

impl Drop for AdmitGuard {
    fn drop(&mut self) {
        agentmux::relay::test_hooks::disarm_send_admit_gate();
        agentmux::relay::test_hooks::disarm_send_admit_commit_gate();
        agentmux::relay::test_hooks::disarm_send_admit_reserved_gate();
        agentmux::relay::test_hooks::disarm_send_published_hook();
        agentmux::relay::test_hooks::disarm_shutdown_wait_hook();
        agentmux::relay::test_hooks::disarm_store_mutation_wait_hook();
    }
}

/// Serializes the global admit-gate window across parked tests: arming
/// replaces the single installed gate, so two overlapping parks would drop
/// the first test's rendezvous channels. Each parked test holds this leaf
/// mutex from arming through joining; it is never acquired while holding
/// the gate, the store lock, or the admission guard.
static PARK_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
