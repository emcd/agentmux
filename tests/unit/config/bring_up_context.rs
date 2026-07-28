//! Bring-up context stamped onto agent-spawning members at configuration load.
//!
//! Bring-up is the only party which authoritatively knows which bundle and
//! session it is starting. Stamping that context onto the member's merged
//! environment is what lets a launched agent propagate it to its
//! `agentmux host mcp` subprocess, so association is carried rather than
//! inferred from the filesystem.

use agentmux::configuration::ConfigurationRoots;
use tempfile::TempDir;

use agentmux::configuration::{
    BUNDLE_ENVIRONMENT_VARIABLE, BringUpContext, BundleMember, SESSION_ENVIRONMENT_VARIABLE,
    load_bundle_configuration,
};

use super::helpers::*;

/// Looks up a merged environment value by name on a resolved member.
fn value_of<'member>(member: &'member BundleMember, name: &str) -> Option<&'member str> {
    member
        .environment
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value.as_str())
}

const ACP_CODER: &str = r#"
format-version = 1

[[coders]]
id = "acp"

[[coders.environment]]
name = "SHARED_DEFAULT"
value = "same-for-everyone"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#;

/// Writes a bundle whose single coder-backed session declares no environment of
/// its own, so the only distinguishing entries are the stamped ones.
fn write_plain_bundle(
    temporary: &TempDir,
    bundle_name: &str,
    session_id: &str,
) -> ConfigurationRoots {
    let dir = temporary.path().display().to_string();
    write_config(
        temporary,
        bundle_name,
        ACP_CODER,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "{session_id}"
directory = "{dir}"
coder = "acp"
"#
        ),
    )
}

#[test]
fn stamps_hosting_bundle_and_member_id() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_plain_bundle(&temporary, "alpha", "reviewer");

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let member = &loaded.members[0];
    assert_eq!(value_of(member, BUNDLE_ENVIRONMENT_VARIABLE), Some("alpha"));
    assert_eq!(
        value_of(member, SESSION_ENVIRONMENT_VARIABLE),
        Some("reviewer")
    );
}

#[test]
fn members_in_different_bundles_carry_their_own_context() {
    // The defect this change exists to fix: a session whose worktree lives
    // outside its bundle's tree inherited the wrong bundle. Two bundles sharing
    // an identical coder-level default must still stamp distinct context, so
    // the shared layer cannot be what carries identity.
    let temporary = TempDir::new().expect("temporary");
    let root = write_plain_bundle(&temporary, "alpha", "reviewer");
    write_plain_bundle(&temporary, "beta", "reviewer");

    let alpha = load_bundle_configuration(&root, "alpha").expect("load alpha");
    let beta = load_bundle_configuration(&root, "beta").expect("load beta");

    assert_eq!(
        value_of(&alpha.members[0], "SHARED_DEFAULT"),
        value_of(&beta.members[0], "SHARED_DEFAULT")
    );
    assert_eq!(
        value_of(&alpha.members[0], BUNDLE_ENVIRONMENT_VARIABLE),
        Some("alpha")
    );
    assert_eq!(
        value_of(&beta.members[0], BUNDLE_ENVIRONMENT_VARIABLE),
        Some("beta")
    );
}

#[test]
fn preserves_operator_declared_context_value() {
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "reviewer"
directory = "{dir}"
coder = "acp"

[[sessions.environment]]
name = "AGENTMUX_BUNDLE"
value = "operator-choice"
"#
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let member = &loaded.members[0];
    // Upsert-if-absent: declaring the name in configuration overrides what
    // bring-up would otherwise supply.
    assert_eq!(
        value_of(member, BUNDLE_ENVIRONMENT_VARIABLE),
        Some("operator-choice")
    );
    // The undeclared companion is still stamped.
    assert_eq!(
        value_of(member, SESSION_ENVIRONMENT_VARIABLE),
        Some("reviewer")
    );
    // Preserved in place rather than duplicated.
    assert_eq!(
        member
            .environment
            .iter()
            .filter(|entry| entry.name == BUNDLE_ENVIRONMENT_VARIABLE)
            .count(),
        1
    );
}

#[test]
fn skips_members_which_spawn_no_agent() {
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &format!(
            r#"
format-version = 1

[[sessions]]
id = "console"
directory = "{dir}"

[sessions.ui]

[[sessions]]
id = "feed"
directory = "{dir}"

[sessions.pubsub]
"#
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members.len(), 2);
    for member in &loaded.members {
        for name in BringUpContext::VARIABLE_NAMES {
            assert_eq!(
                value_of(member, name),
                None,
                "member '{}' spawns no agent and must carry no context",
                member.id
            );
        }
    }
}

#[test]
fn enumerated_names_match_stamped_entries() {
    // The sanitizer which clears inherited context from test children reads
    // `VARIABLE_NAMES`, while the loader stamps `environment_entries`. Holding
    // the two in agreement here is what lets the context be extended without
    // silently leaving a variable unhandled by one of them.
    let context = BringUpContext {
        bundle_name: "alpha",
        session_id: "reviewer",
    };
    let stamped: Vec<&str> = context
        .environment_entries()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(stamped, BringUpContext::VARIABLE_NAMES);
}
