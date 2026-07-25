use std::{path::PathBuf, process::Command};

use agentmux::{
    configuration::BundleConfiguration,
    runtime::association::{
        AssociationCandidates, McpAssociationCli, McpAssociationEnvironment,
        McpAssociationOverrides, WorkspaceContext, load_local_mcp_overrides, resolve_association,
        resolve_sender_session, validate_sender_session,
    },
};
use tempfile::TempDir;

fn run_git(directory: &std::path::Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .args(arguments)
        .output()
        .expect("run git");
    if output.status.success() {
        return;
    }
    panic!(
        "git command failed: git {} \nstdout:\n{}\nstderr:\n{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn context(
    current_directory: &str,
    workspace_root: &str,
    git_top_level: Option<&str>,
    git_common_dir: Option<&str>,
) -> WorkspaceContext {
    WorkspaceContext {
        current_directory: PathBuf::from(current_directory),
        workspace_root: PathBuf::from(workspace_root),
        git_top_level: git_top_level.map(PathBuf::from),
        git_common_dir: git_common_dir.map(PathBuf::from),
    }
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
                        wedge_detection: true,
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
                            wedge_detection: true,
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
fn injected_environment_outranks_the_overlay_and_default_bundle() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("overlay-bundle".to_string()),
        session_name: Some("overlay-session".to_string()),
    };
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &injected(Some("injected-bundle"), Some("injected-session")),
        Some(&overrides),
        Some("default-bundle"),
    );
    assert_eq!(candidates.bundle_name.as_deref(), Some("injected-bundle"));
    assert_eq!(candidates.session_name.as_deref(), Some("injected-session"));
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
    assert_eq!(candidates.bundle_name.as_deref(), Some("cli-bundle"));
    assert_eq!(candidates.session_name.as_deref(), Some("cli-session"));
}

#[test]
fn overlay_outranks_default_bundle() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("overlay-bundle".to_string()),
        session_name: None,
    };
    let candidates = resolve_association(
        &McpAssociationCli::default(),
        &McpAssociationEnvironment::default(),
        Some(&overrides),
        Some("default-bundle"),
    );
    assert_eq!(candidates.bundle_name.as_deref(), Some("overlay-bundle"));
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
    assert_eq!(candidates.bundle_name.as_deref(), Some("default-bundle"));
    assert_eq!(candidates.session_name, None);
}

#[test]
fn blank_values_are_absent_rather_than_present_and_empty() {
    let overrides = McpAssociationOverrides {
        bundle_name: Some("overlay-bundle".to_string()),
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
    assert_eq!(candidates.bundle_name.as_deref(), Some("overlay-bundle"));
    assert_eq!(candidates.session_name, None);
}

#[test]
fn debug_repository_root_prefers_git_common_dir_parent() {
    let workspace = context(
        "/home/me/src/WORKTREES/agentmux/tui",
        "/home/me/src/WORKTREES/agentmux/tui",
        Some("/home/me/src/WORKTREES/agentmux/tui"),
        Some("/home/me/src/agentmux/.git"),
    );
    assert_eq!(
        workspace.debug_repository_root(),
        Some(PathBuf::from("/home/me/src/agentmux"))
    );
}

#[test]
fn debug_repository_root_handles_nested_common_dir_layout() {
    let workspace = context(
        "/home/me/src/WORKTREES/agentmux/tui",
        "/home/me/src/WORKTREES/agentmux/tui",
        Some("/home/me/src/WORKTREES/agentmux/tui"),
        Some("/home/me/src/agentmux/.git/worktrees/tui"),
    );
    assert_eq!(
        workspace.debug_repository_root(),
        Some(PathBuf::from("/home/me/src/agentmux"))
    );
}

#[test]
fn debug_repository_root_is_none_without_git_common_dir() {
    let workspace = context(
        "/home/me/src/WORKTREES/agentmux/tui",
        "/home/me/src/WORKTREES/agentmux/tui",
        Some("/home/me/src/WORKTREES/agentmux/tui"),
        None,
    );
    assert_eq!(workspace.debug_repository_root(), None);
}

#[test]
fn applies_cli_precedence_over_local_overrides() {
    let workspace = context(
        "/home/me/src/WORKTREES/agentmux/relay",
        "/home/me/src/WORKTREES/agentmux/relay",
        Some("/home/me/src/WORKTREES/agentmux/relay"),
        Some("/home/me/src/agentmux/.git"),
    );
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
    assert_eq!(candidates.bundle_name.as_deref(), Some("cli-bundle"));
    assert_eq!(candidates.session_name.as_deref(), Some("cli-session"));
    let _ = &workspace;
}

#[test]
fn loads_association_file_from_the_configuration_root() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path();
    std::fs::write(
        configuration_root.join("mcp.toml"),
        "bundle_name = 'agentmux'\nsession_name = 'relay'\n",
    )
    .expect("write association file");

    let loaded = load_local_mcp_overrides(configuration_root).expect("load overrides");
    let Some(loaded) = loaded else {
        panic!("expected association file");
    };
    assert_eq!(loaded.bundle_name.as_deref(), Some("agentmux"));
    assert_eq!(loaded.session_name.as_deref(), Some("relay"));
}

#[test]
fn association_overlay_shadows_the_base_file() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path();
    std::fs::create_dir_all(configuration_root.join("overlay")).expect("create overlay");
    std::fs::write(
        configuration_root.join("mcp.toml"),
        "bundle_name = 'base-bundle'\n",
    )
    .expect("write base association file");
    std::fs::write(
        configuration_root.join("overlay/mcp.toml"),
        "bundle_name = 'overlay-bundle'\n",
    )
    .expect("write overlay association file");

    let loaded = load_local_mcp_overrides(configuration_root)
        .expect("load overrides")
        .expect("expected association file");
    assert_eq!(loaded.bundle_name.as_deref(), Some("overlay-bundle"));
}

#[test]
fn rejects_malformed_local_override_file() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    std::fs::write(
        root.join("mcp.toml"),
        "bundle_name = 'agentmux'\nunknown_field = 1\n",
    )
    .expect("write override");

    let err = load_local_mcp_overrides(root).expect_err("override should fail");
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
fn resolves_sender_from_working_directory_when_candidate_is_unknown() {
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
        resolve_sender_session(&bundle, "relay", relay_directory.as_path()).expect("resolve");
    assert_eq!(resolved, "master");
}

#[test]
fn rejects_unknown_sender_when_directory_does_not_match_any_member() {
    let temporary = TempDir::new().expect("temporary");
    let relay_directory = temporary.path().join("relay");
    let other_directory = temporary.path().join("other");
    std::fs::create_dir_all(&relay_directory).expect("create relay directory");
    std::fs::create_dir_all(&other_directory).expect("create other directory");
    let bundle = bundle_with_directories(&[
        ("master", relay_directory.as_path()),
        ("other", other_directory.as_path()),
    ]);

    let unknown_directory = temporary.path().join("unknown");
    std::fs::create_dir_all(&unknown_directory).expect("create unknown directory");
    let err = resolve_sender_session(&bundle, "relay", unknown_directory.as_path())
        .expect_err("unknown sender should fail");
    assert!(err.to_string().contains("validation_unknown_sender"));
    assert!(
        err.to_string()
            .contains("did not match any configured session directory"),
        "unexpected error: {err}"
    );
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

    let err = resolve_sender_session(&bundle, "relay", relay_directory.as_path())
        .expect_err("ambiguous sender should fail");
    assert!(err.to_string().contains("validation_unknown_sender"));
    assert!(
        err.to_string()
            .contains("matched multiple configured sessions")
    );
}

#[test]
fn linked_worktree_resolves_the_common_dir_owner_repository_root() {
    let temporary = TempDir::new().expect("temporary");
    let project_root = temporary.path().join("agentmux");
    std::fs::create_dir_all(&project_root).expect("create project root");

    run_git(&project_root, &["init"]);
    run_git(
        &project_root,
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test User",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );

    let worktree_root = temporary.path().join("WORKTREES/agentmux/relay");
    std::fs::create_dir_all(
        worktree_root
            .parent()
            .expect("worktree parent should exist"),
    )
    .expect("create worktree parent");
    run_git(
        &project_root,
        &[
            "worktree",
            "add",
            "--detach",
            worktree_root.to_str().expect("utf8 path"),
        ],
    );

    // Association no longer derives anything from Git. What remains is the
    // repository root feeding repository-local state and inscriptions, and it
    // must resolve to the common-dir *owner* rather than the linked worktree, so
    // every worktree of a checkout shares one relay rather than starting its own.
    let discovered = WorkspaceContext::discover(&worktree_root).expect("discover workspace");
    let repository_root = discovered
        .debug_repository_root()
        .expect("linked worktree must resolve a repository root");
    assert_eq!(
        repository_root
            .canonicalize()
            .expect("canonicalize resolved"),
        project_root
            .canonicalize()
            .expect("canonicalize project root"),
    );
}
