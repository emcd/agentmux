use std::{path::PathBuf, process::Command};

use tempfile::TempDir;
use tmuxmux::{
    configuration::BundleConfiguration,
    runtime::association::{
        McpAssociationCli, McpAssociationOverrides, WorkspaceContext, load_local_mcp_overrides,
        resolve_association, validate_sender_session,
    },
};

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
        bundle_name: "tmuxmux".to_string(),
        members: sessions
            .iter()
            .map(|session_name| tmuxmux::configuration::BundleMember {
                id: (*session_name).to_string(),
                name: None,
                working_directory: None,
                start_command: None,
                prompt_readiness: None,
            })
            .collect(),
    }
}

#[test]
fn resolves_auto_association_from_git_context() {
    let workspace = context(
        "/home/me/src/WORKTREES/tmuxmux/relay",
        "/home/me/src/WORKTREES/tmuxmux/relay",
        Some("/home/me/src/WORKTREES/tmuxmux/relay"),
        Some("/home/me/src/tmuxmux/.git"),
    );
    let resolved =
        resolve_association(&McpAssociationCli::default(), None, &workspace).expect("association");
    assert_eq!(resolved.bundle_name, "tmuxmux");
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
fn applies_cli_precedence_over_local_overrides() {
    let workspace = context(
        "/home/me/src/WORKTREES/tmuxmux/relay",
        "/home/me/src/WORKTREES/tmuxmux/relay",
        Some("/home/me/src/WORKTREES/tmuxmux/relay"),
        Some("/home/me/src/tmuxmux/.git"),
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
    let override_directory = root.join(".auxiliary/configuration/tmuxmux/overrides");
    std::fs::create_dir_all(&override_directory).expect("create override dir");
    std::fs::write(
        override_directory.join("mcp.toml"),
        "bundle_name = 'tmuxmux'\nsession_name = 'relay'\nconfig_root = '../shared-config'\n",
    )
    .expect("write override");

    let loaded = load_local_mcp_overrides(root).expect("load overrides");
    let Some(loaded) = loaded else {
        panic!("expected override file");
    };
    assert_eq!(loaded.bundle_name.as_deref(), Some("tmuxmux"));
    assert_eq!(loaded.session_name.as_deref(), Some("relay"));
    assert_eq!(loaded.config_root, Some(root.join("../shared-config")));
}

#[test]
fn rejects_malformed_local_override_file() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let override_directory = root.join(".auxiliary/configuration/tmuxmux/overrides");
    std::fs::create_dir_all(&override_directory).expect("create override dir");
    std::fs::write(
        override_directory.join("mcp.toml"),
        "bundle_name = 'tmuxmux'\nunknown_field = 1\n",
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
fn discovers_bundle_and_session_from_real_git_worktree() {
    let temporary = TempDir::new().expect("temporary");
    let project_root = temporary.path().join("tmuxmux");
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

    let worktree_root = temporary.path().join("WORKTREES/tmuxmux/relay");
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

    assert_eq!(bundle_name, "tmuxmux");
    assert_eq!(session_name, "relay");
}
