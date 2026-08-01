use agentmux::configuration::ConfigurationRoots;
use agentmux::{
    configuration::BundleConfiguration,
    runtime::association::{
        AssociationCandidate, AssociationCandidates, AssociationSource, McpAssociationCli,
        McpAssociationEnvironment, McpAssociationOverrides, load_local_mcp_overrides,
        resolve_association, resolve_sender_session, validate_sender_session,
    },
};
use tempfile::TempDir;

/// The resolved value, dropping the tier that supplied it.
fn named(candidate: Option<&AssociationCandidate>) -> Option<&str> {
    candidate.map(|candidate| candidate.value.as_str())
}

/// The tier that supplied a resolved value.
fn sourced(candidate: Option<&AssociationCandidate>) -> Option<AssociationSource> {
    candidate.map(|candidate| candidate.source)
}

fn bundle_with_sessions(sessions: &[&str]) -> BundleConfiguration {
    BundleConfiguration {
        schema_version: "1".to_string(),
        bundle_name: "agentmux".to_string(),
        autostart: false,
        groups: Vec::new(),
        members: sessions
            .iter()
            .map(|session_name| agentmux::configuration::BundleMember {
                id: (*session_name).to_string(),
                name: None,
                working_directory: None,
                target: agentmux::configuration::TargetConfiguration::Tmux(
                    agentmux::configuration::TmuxTargetConfiguration {
                        start_command: "sh -lc 'true'".to_string(),
                        prompt_readiness: None,
                        prime_timeout_ms: None,
                        readiness_timeout_ms:
                            agentmux::configuration::TMUX_READINESS_TIMEOUT_MS_DEFAULT,
                    },
                ),
                coder_session_id: None,
                policy_id: None,
                environment: Vec::new(),
            })
            .collect(),
    }
}

fn bundle_with_directories(
    session_directories: &[(&str, &std::path::Path)],
) -> BundleConfiguration {
    BundleConfiguration {
        schema_version: "1".to_string(),
        bundle_name: "agentmux".to_string(),
        autostart: false,
        groups: Vec::new(),
        members: session_directories
            .iter()
            .map(
                |(session_name, directory)| agentmux::configuration::BundleMember {
                    id: (*session_name).to_string(),
                    name: None,
                    working_directory: Some((*directory).to_path_buf()),
                    target: agentmux::configuration::TargetConfiguration::Tmux(
                        agentmux::configuration::TmuxTargetConfiguration {
                            start_command: "sh -lc 'true'".to_string(),
                            prompt_readiness: None,
                            prime_timeout_ms: None,
                            readiness_timeout_ms:
                                agentmux::configuration::TMUX_READINESS_TIMEOUT_MS_DEFAULT,
                        },
                    ),
                    coder_session_id: None,
                    policy_id: None,
                    environment: Vec::new(),
                },
            )
            .collect(),
    }
}

/// Association identities carried by the injected bring-up environment.
fn injected(bundle: Option<&str>, session: Option<&str>) -> McpAssociationEnvironment {
    McpAssociationEnvironment {
        bundle_name: bundle.map(ToString::to_string),
        session_name: session.map(ToString::to_string),
    }
}

#[test]
fn nothing_supplied_resolves_to_nothing() {
    // Absence is recorded, never guessed. Filesystem inference produced an
    // answer that was plausible and wrong, which is the defect this replaces.
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &McpAssociationEnvironment::default(),
        None,
        None,
    );
    assert_eq!(candidates, AssociationCandidates::default());
}

#[test]
fn injected_environment_outranks_the_association_file_and_default_bundle() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("file-bundle".to_string()),
        session_name: Some("file-session".to_string()),
    };
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &injected(Some("injected-bundle"), Some("injected-session")),
        Some(&overrides),
        Some("default-bundle"),
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("injected-bundle"));
    assert_eq!(named(candidates.session.as_ref()), Some("injected-session"));
    assert_eq!(
        sourced(candidates.bundle.as_ref()),
        Some(AssociationSource::Environment)
    );
    assert_eq!(
        sourced(candidates.session.as_ref()),
        Some(AssociationSource::Environment)
    );
}

#[test]
fn explicit_flags_outrank_the_injected_environment() {
    let candidates = resolve_association(
        &McpAssociationCli {
            bundle_name: Some("cli-bundle".to_string()),
            session_name: Some("cli-session".to_string()),
        },
        &injected(Some("injected-bundle"), Some("injected-session")),
        None,
        Some("default-bundle"),
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("cli-bundle"));
    assert_eq!(named(candidates.session.as_ref()), Some("cli-session"));
    assert_eq!(
        sourced(candidates.bundle.as_ref()),
        Some(AssociationSource::CommandLine)
    );
}

#[test]
fn the_association_file_outranks_default_bundle() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("file-bundle".to_string()),
        session_name: None,
    };
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &McpAssociationEnvironment::default(),
        Some(&overrides),
        Some("default-bundle"),
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("file-bundle"));
    assert_eq!(
        sourced(candidates.bundle.as_ref()),
        Some(AssociationSource::AssociationFile)
    );
    // The tier's reported name is diagnostic output an operator reads to see
    // which tier supplied an identity, so the string is asserted directly
    // rather than only through the variant.
    assert_eq!(
        AssociationSource::AssociationFile.as_str(),
        "association-file"
    );
}

#[test]
fn default_bundle_applies_when_no_higher_tier_resolves() {
    // The tier that lets generated client configuration seed a bundle without
    // impersonating invocation intent.
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &McpAssociationEnvironment::default(),
        None,
        Some("default-bundle"),
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("default-bundle"));
    assert_eq!(
        sourced(candidates.bundle.as_ref()),
        Some(AssociationSource::DefaultBundle)
    );
    assert_eq!(candidates.session, None);
}

#[test]
fn blank_values_are_absent_rather_than_present_and_empty() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("file-bundle".to_string()),
        session_name: None,
    };
    let candidates = resolve_association(
        &McpAssociationCli {
            bundle_name: Some("   ".to_string()),
            session_name: Some(String::new()),
        },
        &McpAssociationEnvironment::default(),
        Some(&overrides),
        None,
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("file-bundle"));
    assert_eq!(candidates.session, None);
}

#[test]
fn applies_cli_precedence_over_local_overrides() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("override-bundle".to_string()),
        session_name: Some("override-session".to_string()),
    };
    let candidates = resolve_association(
        &McpAssociationCli {
            bundle_name: Some("cli-bundle".to_string()),
            session_name: Some("cli-session".to_string()),
        },
        &McpAssociationEnvironment::default(),
        Some(&overrides),
        None,
    );
    assert_eq!(named(candidates.bundle.as_ref()), Some("cli-bundle"));
    assert_eq!(named(candidates.session.as_ref()), Some("cli-session"));
}

#[test]
fn loads_association_file_from_the_configuration_root() {
    let temporary = TempDir::new().expect("temporary");
    std::fs::write(
        temporary.path().join("mcp.toml"),
        "bundle_name = 'agentmux'\nsession_name = 'relay'\n",
    )
    .expect("write association file");

    let loaded = load_local_mcp_overrides(&ConfigurationRoots::single(temporary.path()))
        .expect("load overrides");
    let Some(loaded) = loaded else {
        panic!("expected association file");
    };
    assert_eq!(loaded.bundle_name.as_deref(), Some("agentmux"));
    assert_eq!(loaded.session_name.as_deref(), Some("relay"));
}

#[test]
fn an_earlier_association_layer_shadows_a_later_one() {
    let temporary = TempDir::new().expect("temporary");
    let base = temporary.path().join("base");
    let override_layer = temporary.path().join("rnd");
    std::fs::create_dir_all(&base).expect("create base layer");
    std::fs::create_dir_all(&override_layer).expect("create override layer");
    std::fs::write(base.join("mcp.toml"), "bundle_name = 'base-bundle'\n")
        .expect("write base association file");
    std::fs::write(
        override_layer.join("mcp.toml"),
        "bundle_name = 'override-bundle'\n",
    )
    .expect("write override association file");

    let roots = ConfigurationRoots::from_elements([override_layer, base]).expect("layer list");
    let loaded = load_local_mcp_overrides(&roots)
        .expect("load overrides")
        .expect("expected association file");
    assert_eq!(loaded.bundle_name.as_deref(), Some("override-bundle"));
}

#[test]
fn an_association_file_in_a_later_layer_is_reached() {
    let temporary = TempDir::new().expect("temporary");
    let base = temporary.path().join("base");
    let override_layer = temporary.path().join("rnd");
    std::fs::create_dir_all(&base).expect("create base layer");
    std::fs::create_dir_all(&override_layer).expect("create override layer");
    std::fs::write(base.join("mcp.toml"), "bundle_name = 'base-bundle'\n")
        .expect("write base association file");

    let roots = ConfigurationRoots::from_elements([override_layer, base]).expect("layer list");
    let loaded = load_local_mcp_overrides(&roots)
        .expect("load overrides")
        .expect("expected association file");
    assert_eq!(loaded.bundle_name.as_deref(), Some("base-bundle"));
}

#[test]
fn rejects_malformed_local_override_file() {
    let temporary = TempDir::new().expect("temporary");
    std::fs::write(
        temporary.path().join("mcp.toml"),
        "bundle_name = 'agentmux'\nunknown_field = 1\n",
    )
    .expect("write override");

    let err = load_local_mcp_overrides(&ConfigurationRoots::single(temporary.path()))
        .expect_err("override should fail");
    assert!(err.to_string().contains("validation_invalid_arguments"));
}

#[test]
fn validates_sender_membership() {
    let bundle = bundle_with_sessions(&["relay", "tui"]);
    let resolved = validate_sender_session(&bundle, "relay").expect("sender");
    assert_eq!(resolved, "relay");
}

#[test]
fn rejects_unknown_sender_membership() {
    let bundle = bundle_with_sessions(&["relay", "tui"]);
    let err = validate_sender_session(&bundle, "planner").expect_err("should fail");
    assert!(err.to_string().contains("validation_unknown_sender"));
}

#[test]
fn resolves_sender_from_working_directory_when_no_candidate_is_supplied() {
    let temporary = TempDir::new().expect("temporary");
    let relay_directory = temporary.path().join("relay");
    let other_directory = temporary.path().join("other");
    std::fs::create_dir_all(&relay_directory).expect("create relay directory");
    std::fs::create_dir_all(&other_directory).expect("create other directory");
    let bundle = bundle_with_directories(&[
        ("master", relay_directory.as_path()),
        ("other", other_directory.as_path()),
    ]);

    let resolved =
        resolve_sender_session(&bundle, None, relay_directory.as_path()).expect("resolve");
    assert_eq!(resolved.as_deref(), Some("master"));
}

#[test]
fn refuses_a_supplied_candidate_which_names_no_member() {
    let temporary = TempDir::new().expect("temporary");
    let relay_directory = temporary.path().join("relay");
    let other_directory = temporary.path().join("other");
    std::fs::create_dir_all(&relay_directory).expect("create relay directory");
    std::fs::create_dir_all(&other_directory).expect("create other directory");
    let bundle = bundle_with_directories(&[
        ("master", relay_directory.as_path()),
        ("other", other_directory.as_path()),
    ]);

    // The working directory *does* match `master`. Accepting it here would let a
    // mistyped selector authenticate as whichever member owns the directory,
    // reintroducing identity by inference one tier below the Git guessing this
    // ladder replaced.
    let err = resolve_sender_session(&bundle, Some("relay"), relay_directory.as_path())
        .expect_err("a supplied candidate naming no member must be refused");
    assert!(err.to_string().contains("validation_unknown_sender"));
    assert!(
        !err.to_string().contains("master"),
        "refusal must not name the member it declined to become: {err}"
    );
}

#[test]
fn reports_no_sender_when_nothing_supplied_and_no_directory_matches() {
    let temporary = TempDir::new().expect("temporary");
    let relay_directory = temporary.path().join("relay");
    std::fs::create_dir_all(&relay_directory).expect("create relay directory");
    let bundle = bundle_with_directories(&[("master", relay_directory.as_path())]);

    let unknown_directory = temporary.path().join("unknown");
    std::fs::create_dir_all(&unknown_directory).expect("create unknown directory");
    // Nothing was supplied, so nothing was mistaken: an unassociated server is a
    // legitimate outcome rather than a fault.
    let resolved =
        resolve_sender_session(&bundle, None, unknown_directory.as_path()).expect("resolve");
    assert_eq!(resolved, None);
}

#[test]
fn rejects_ambiguous_sender_when_directory_matches_multiple_members() {
    let temporary = TempDir::new().expect("temporary");
    let relay_directory = temporary.path().join("relay");
    std::fs::create_dir_all(&relay_directory).expect("create relay directory");
    let bundle = bundle_with_directories(&[
        ("master", relay_directory.as_path()),
        ("shadow", relay_directory.as_path()),
    ]);

    let err = resolve_sender_session(&bundle, None, relay_directory.as_path())
        .expect_err("ambiguous sender should fail");
    assert!(err.to_string().contains("validation_unknown_sender"));
    assert!(
        err.to_string()
            .contains("matched multiple configured sessions")
    );
}
