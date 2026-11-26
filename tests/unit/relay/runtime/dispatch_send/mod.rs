//! In-process send dispatch through the public handler contract.
//!
//! Matrix coverage lives in [`fusion`]; final-admission interleavings live
//! in [`admission`]. Shared fixtures and verified-context helpers live here.

use agentmux::relay::{
    DeliveryConfiguration, RelayResponse, RelayRuntime, VerifiedPrincipal,
};
use tempfile::TempDir;

pub(super) use super::dispatch_fixtures::{
    SendFixture, provisioned_psk, send_fixture, send_fixture_with_members,
    verified_context,
};

mod admission;
mod fusion;
mod store_hold;
mod wiring;
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
            // Hosted bundles declare autostart explicitly: an absent key
            // means Hold, so fixtures model running bundles honestly.
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
    let runtime = std::sync::Arc::new(
        RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            DeliveryConfiguration::default(),
        )
        .expect("runtime init"),
    );
    ParkFixture {
        _temporary: temporary,
        runtime,
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
