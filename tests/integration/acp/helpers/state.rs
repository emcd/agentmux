use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::RelayResponse;
use agentmux::runtime::paths::BundleRuntimePaths;
use std::path::{Path, PathBuf};

use super::stub::ensure_fast_respawn_for_tests;
pub(in crate::acp) fn flat_bundle_paths(root: &Path) -> BundleRuntimePaths {
    BundleRuntimePaths {
        state_root: root.to_path_buf(),
        bundle_name: "party".to_string(),
        runtime_directory: root.to_path_buf(),
        tmux_socket: root.join("tmux.sock"),
    }
}
pub(in crate::acp) fn startup_bundle(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<(), agentmux::relay::RelayError> {
    ensure_fast_respawn_for_tests();
    let runtime_directory = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    let mut paths = flat_bundle_paths(runtime_directory);
    paths.tmux_socket = tmux_socket.to_path_buf();
    let _ = agentmux::relay::startup_bundle(config_root, &paths)?;
    Ok(())
}

pub(in crate::acp) fn persisted_state_path(root: &Path, target_session: &str) -> PathBuf {
    root.join("sessions")
        .join(target_session)
        .join("state.json")
}

pub(in crate::acp) fn read_worker_state(root: &Path, target_session: &str) -> Option<String> {
    agentmux::relay::read_worker_readiness("party", root, target_session).map(ToString::to_string)
}

pub(in crate::acp) fn send_result(response: RelayResponse) -> agentmux::relay::SendResult {
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    results.into_iter().next().expect("one result")
}
