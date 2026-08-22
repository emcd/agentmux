//! Bring-up context stamped onto agent-spawning members at configuration load.
//!
//! Bring-up is the only party which authoritatively knows which bundle and
//! session it is starting. Stamping that context onto the member's merged
//! environment is what lets a launched agent propagate it to its
//! `agentmux host mcp` subprocess, so association is carried rather than
//! inferred from the filesystem.

use std::path::PathBuf;

use agentmux::configuration::ConfigurationRoots;
use tempfile::TempDir;

use agentmux::configuration::{
    BUNDLE_ENVIRONMENT_VARIABLE, BringUpContext, BundleMember,
    CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE, ContextValue, INHERITED_CONTEXT_VARIABLE_NAMES,
    LayerRepresentationFault, SESSION_ENVIRONMENT_VARIABLE, load_bundle_configuration,
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

/// Writes a bundle under a configuration root named by `root_name`, so a caller
/// can choose which representation fault the resolved list carries. The bundle
/// body is supplied so a caller can also vary whether any member would be
/// stamped; the two axes are independent and the rule reads on both.
fn write_bundle_under_root_named(
    temporary: &TempDir,
    root_name: &std::ffi::OsStr,
    bundle_name: &str,
    bundle_toml: &str,
) -> ConfigurationRoots {
    let root = temporary.path().join(root_name);
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create directories");
    std::fs::write(root.join("coders.toml"), ACP_CODER).expect("write coders");
    std::fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml).expect("write bundle");
    ConfigurationRoots::single(root)
}

/// A root whose path holds the layer separator.
fn separator_root_name() -> &'static std::ffi::OsStr {
    std::ffi::OsStr::new("holds:separator")
}

/// A root whose path is not valid Unicode, where one can exist on disk.
///
/// `None` on macOS. APFS enforces valid UTF-8 in path components, so creating
/// this root fails with `EILSEQ` before the loader is reached; the scenario is
/// not awkward to set up there, it cannot exist on that filesystem at all.
/// Linux treats a path as opaque bytes and accepts it.
fn not_unicode_root_name() -> Option<&'static std::ffi::OsStr> {
    use std::os::unix::ffi::OsStrExt;

    if cfg!(target_os = "macos") {
        return None;
    }
    Some(std::ffi::OsStr::from_bytes(b"not\xffunicode"))
}

/// Every representation fault a root name can carry on this platform, paired
/// with the fragment a rejection names it by.
///
/// Crossing these with the bundle shapes below covers each combination the
/// filesystem can hold. Where the non-Unicode name is unavailable, the fault
/// itself stays covered by the rendering tests, which need no directory, and
/// what the loader does with a fault stays covered by the separator name.
fn unrepresentable_root_names() -> Vec<(&'static std::ffi::OsStr, &'static str)> {
    let mut names = vec![(separator_root_name(), "contains ':'")];
    if let Some(not_unicode) = not_unicode_root_name() {
        names.push((not_unicode, "not valid Unicode"));
    }
    names
}

/// A bundle with one coder-backed member declaring no environment of its own,
/// so the relay's layer list is what it would receive.
fn stamped_member_bundle(directory: &str) -> String {
    format!(
        r#"
format-version = 1

[[sessions]]
id = "reviewer"
directory = "{directory}"
coder = "acp"
"#
    )
}

/// A bundle with one coder-backed member declaring its own layer list, so the
/// relay's is never rendered for it.
fn declaring_member_bundle(directory: &str) -> String {
    format!(
        r#"
format-version = 1

[[sessions]]
id = "reviewer"
directory = "{directory}"
coder = "acp"

[[sessions.environment]]
name = "AGENTMUX_CONFIGURATION_DIRECTORY"
value = "/operator/choice"
"#
    )
}

/// A bundle whose only member spawns no agent, so nothing is stamped.
fn coder_less_bundle(directory: &str) -> String {
    format!(
        r#"
format-version = 1

[[sessions]]
id = "feed"
directory = "{directory}"

[sessions.pubsub]
"#
    )
}

#[test]
fn stamps_the_relays_configuration_layer_list() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_plain_bundle(&temporary, "alpha", "reviewer");

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    let member = &loaded.members[0];
    assert_eq!(
        value_of(member, CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE),
        Some(root.layers()[0].display().to_string().as_str()),
        "a coder-backed member reads the declarations of the relay that spawned it"
    );
}

#[test]
fn preserves_an_operator_declared_configuration_layer_list() {
    // Unlike the state root, this is upsert-if-absent: an operator-declared
    // value is a preference rather than a broken rendezvous, because the socket
    // and credentials resolve beneath the state root regardless.
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
name = "AGENTMUX_CONFIGURATION_DIRECTORY"
value = "/operator/choice"
"#
        ),
    );

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(
        value_of(
            &loaded.members[0],
            CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE
        ),
        Some("/operator/choice")
    );
}

#[test]
fn every_stamped_name_is_also_sanitized_from_an_inherited_environment() {
    // The omission this change exists to fix, stated as the invariant rather
    // than as the one variable that broke it. A name that load stamps but a
    // sanitizing consumer does not clear leaks the developer's own value into
    // whatever the harness spawns, and nothing else fails to say so.
    for name in BringUpContext::VARIABLE_NAMES {
        assert!(
            INHERITED_CONTEXT_VARIABLE_NAMES.contains(name),
            "'{name}' is stamped at load but absent from the inherited-context \
             sanitization set, so a harness clearing inherited context leaves it in place"
        );
    }
}

#[test]
fn rendering_joins_every_layer_in_declared_order() {
    // The stamped value is a search path, so a member reading it has to find
    // the layers in the precedence the relay resolved -- an override ahead of
    // the base it overrides.
    let roots = ConfigurationRoots::from_elements(["/override", "/base"].map(PathBuf::from))
        .expect("layer list");
    assert_eq!(
        ContextValue::Layers(&roots).render().expect("render"),
        "/override:/base"
    );
}

#[test]
fn rendering_reports_an_unrepresentable_layer_rather_than_approximating_it() {
    // Both faults are properties of the rendering, not of any filesystem, so
    // they are stated here without a directory having to exist. That is what
    // keeps the non-Unicode fault covered on a filesystem which cannot hold
    // such a path; the loader tests below reach only the faults their platform
    // can construct.
    use std::os::unix::ffi::OsStrExt;

    for (layer, expected) in [
        (
            PathBuf::from(std::ffi::OsStr::from_bytes(b"/not\xffunicode")),
            LayerRepresentationFault::NotUnicode,
        ),
        (
            PathBuf::from("/holds:separator"),
            LayerRepresentationFault::HoldsSeparator,
        ),
    ] {
        let roots = ConfigurationRoots::single(layer.clone());
        let Err(unrepresentable) = ContextValue::Layers(&roots).render() else {
            panic!("{layer:?}: an unrepresentable layer must not render");
        };
        assert_eq!(
            unrepresentable.fault, expected,
            "{layer:?}: the fault names why the layer cannot be represented"
        );
        assert_eq!(
            unrepresentable.layer, layer,
            "{layer:?}: the offending layer is carried so a caller can name it"
        );
    }
}

#[test]
fn an_unrepresentable_layer_is_rejected_where_a_member_would_be_stamped() {
    // Neither fault has a faithful rendering. Forcing one would stamp a path
    // naming a directory the relay never read -- by substituting replacement
    // characters for undecodable bytes, or by splitting on an embedded
    // separator -- and the member would resolve it with nothing reporting the
    // difference. That is the silent divergence this stamp exists to close,
    // reintroduced by the stamp itself.
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();

    for (root_name, expected_fault) in unrepresentable_root_names() {
        let root = write_bundle_under_root_named(
            &temporary,
            root_name,
            "alpha",
            &stamped_member_bundle(&dir),
        );

        let Err(error) = load_bundle_configuration(&root, "alpha") else {
            panic!("{root_name:?}: an unrepresentable layer must not be stamped");
        };
        let rendered = error.to_string();
        assert!(
            rendered.contains(expected_fault),
            "{root_name:?}: the error names which fault the layer carries, got {rendered:?}"
        );
    }
}

#[test]
fn an_unrepresentable_layer_loads_when_no_member_would_be_stamped() {
    // A coder-less member spawns no agent and is never stamped, so nothing
    // needs the environment representation and the configuration is not faulty.
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();

    for (root_name, _) in unrepresentable_root_names() {
        let root =
            write_bundle_under_root_named(&temporary, root_name, "alpha", &coder_less_bundle(&dir));

        let loaded = load_bundle_configuration(&root, "alpha")
            .unwrap_or_else(|error| panic!("{root_name:?}: {error}"));
        assert_eq!(loaded.members.len(), 1);
        assert_eq!(
            value_of(
                &loaded.members[0],
                CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE
            ),
            None,
            "{root_name:?}: a coder-less member carries no stamped context"
        );
    }
}

#[test]
fn an_unrepresentable_layer_loads_when_the_member_declares_its_own() {
    // This is the case an eager render fails, for either fault: the relay's
    // list is never rendered for a member that already declares the name, so
    // the unrepresentable layer is never reached. Rendering during entry
    // enumeration -- or anywhere ahead of the presence check -- rejects this.
    let temporary = TempDir::new().expect("temporary");
    let dir = temporary.path().display().to_string();

    for (root_name, _) in unrepresentable_root_names() {
        let root = write_bundle_under_root_named(
            &temporary,
            root_name,
            "alpha",
            &declaring_member_bundle(&dir),
        );

        let loaded = load_bundle_configuration(&root, "alpha")
            .unwrap_or_else(|error| panic!("{root_name:?}: {error}"));
        assert_eq!(
            value_of(
                &loaded.members[0],
                CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE
            ),
            Some("/operator/choice"),
            "{root_name:?}: a declared value survives regardless of the fault"
        );
    }
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
    let roots = ConfigurationRoots::single("/configuration");
    let context = BringUpContext {
        bundle_name: "alpha",
        session_id: "reviewer",
        configuration_roots: &roots,
    };
    let stamped: Vec<&str> = context
        .environment_entries()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(stamped, BringUpContext::VARIABLE_NAMES);
}
