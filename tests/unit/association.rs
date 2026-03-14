use std::{path::PathBuf, process::Command};

use agentmux::{
    configuration::BundleConfiguration,
    runtime::association::{
        McpAssociationCli, McpAssociationOverrides, WorkspaceContext, load_local_mcp_overrides,
        resolve_association, resolve_sender_session, validate_sender_session,
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
        groups: Vec::new(),
        members: sessions
            .iter()
            .map(|session_name| agentmux::configuration::BundleMember {
                id: (*session_name).to_string(),
                name: None,
                working_directory: None,
                start_command: None,
                prompt_readiness: None,
                policy_id: None,
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
        groups: Vec::new(),
        members: session_directories
            .iter()
            .map(
                |(session_name, directory)| agentmux::configuration::BundleMember {
                    id: (*session_name).to_string(),
                    name: None,
                    working_directory: Some((*directory).to_path_buf()),
                    start_command: None,
                    prompt_readiness: None,
                    policy_id: None,
                },
            )
            .collect(),
    }
}

#[test]
fn resolves_auto_association_from_git_context() {
    let workspace = context(
        "/home/me/src/WORKTREES/agentmux/relay",
        "/home/me/src/WORKTREES/agentmux/relay",
        Some("/home/me/src/WORKTREES/agentmux/relay"),
        Some("/home/me/src/agentmux/.git"),
    );
    let resolved =
        resolve_association(&McpAssociationCli::default(), None, &workspace).expect("association");
    assert_eq!(resolved.bundle_name, "agentmux");
    assert_eq!(resolved.session_name, "relay");
}

#[test]
fn resolves_auto_association_from_non_git_context() {
    let workspace = context(
        "/home/me/src/project-alpha",
        "/home/me/src/project-alpha",
        None,
        None,
    );
    let resolved =
        resolve_association(&McpAssociationCli::default(), None, &workspace).expect("association");
    assert_eq!(resolved.bundle_name, "project-alpha");
    assert_eq!(resolved.session_name, "project-alpha");
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
        config_root: None,
    };
    let resolved = resolve_association(
        &McpAssociationCli {
            bundle_name: Some("cli-bundle".to_string()),
            session_name: Some("cli-session".to_string()),
        },
        Some(&overrides),
        &workspace,
    )
    .expect("association");
    assert_eq!(resolved.bundle_name, "cli-bundle");
    assert_eq!(resolved.session_name, "cli-session");
}

#[test]
fn loads_local_override_file_and_normalizes_relative_config_root() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let override_directory = root.join(".auxiliary/configuration/agentmux/overrides");
    std::fs::create_dir_all(&override_directory).expect("create override dir");
    std::fs::write(
        override_directory.join("mcp.toml"),
        "bundle_name = 'agentmux'\nsession_name = 'relay'\nconfig_root = '../shared-config'\n",
    )
    .expect("write override");

    let loaded = load_local_mcp_overrides(root).expect("load overrides");
    let Some(loaded) = loaded else {
        panic!("expected override file");
    };
    assert_eq!(loaded.bundle_name.as_deref(), Some("agentmux"));
    assert_eq!(loaded.session_name.as_deref(), Some("relay"));
    assert_eq!(loaded.config_root, Some(root.join("../shared-config")));
}

#[test]
fn rejects_malformed_local_override_file() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let override_directory = root.join(".auxiliary/configuration/agentmux/overrides");
    std::fs::create_dir_all(&override_directory).expect("create override dir");
    std::fs::write(
        override_directory.join("mcp.toml"),
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
fn discovers_bundle_and_session_from_real_git_worktree() {
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

    let discovered = WorkspaceContext::discover(&worktree_root).expect("discover workspace");
    let bundle_name = discovered.auto_bundle_name().expect("auto bundle");
    let session_name = discovered.auto_session_name().expect("auto session");

    assert_eq!(bundle_name, "agentmux");
    assert_eq!(session_name, "relay");
}
