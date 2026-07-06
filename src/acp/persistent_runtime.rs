//! Per-target ACP runtime lifecycle: bootstrap (initial spawn + initialize +
//! lifecycle selection) and the persistent [`PersistentAcpWorkerRuntime`]
//! handle it returns.
//!
//! Extracted from `crate::acp::transport` so the transport module can focus on
//! per-call delivery mechanics (write channel, prime timer, outcome mapping) and
//! so the bootstrap path sits next to its companions
//! ([`crate::acp::state::load_persisted_acp_session_id`] and
//! [`crate::acp::state::persist_acp_session_id`]) without crossing the
//! transport-module boundary.

use std::path::Path;

use serde_json::Value;

use crate::configuration::{AcpChannel, AcpTargetConfiguration, BundleMember, TargetConfiguration};

use super::client::AcpStdioClient;

/// Bootstrap initialize failure; surfaced to the worker's respawn classifier.
pub const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
/// Capability-gap detected at bootstrap; failures with this code are
/// permanent (respawn cannot resolve a missing capability).
const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

/// The persistent ACP runtime owned by an [`super::transport::AcpTransport`]:
/// the stdio client and the resolved session id used for every prompt.
pub struct PersistentAcpWorkerRuntime {
    pub client: AcpStdioClient,
    pub session_id: String,
}

/// A structured bootstrap failure. Surfaced to the relay worker so it can
/// decide whether respawn might recover or the failure is permanent.
#[derive(Clone, Debug)]
pub struct AcpBootstrapError {
    pub code: String,
    pub reason: String,
}

impl AcpBootstrapError {
    /// Permanent failures are conditions respawn cannot resolve: a capability
    /// gap means the agent fundamentally cannot host the session, so retrying
    /// with the same binary reproduces the failure.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.code == ACP_ERROR_CODE_MISSING_CAPABILITY
    }
}

#[derive(Clone, Copy, Debug)]
enum AcpLifecycleSelection {
    NewSession,
    LoadSession,
}

#[derive(Clone, Debug)]
struct AcpCapabilities {
    load_session: bool,
    prompt_session: bool,
}

/// Builds the per-target ACP runtime. Used by the relay worker for initial
/// bootstrap and respawn (the worker re-publishes the [`OutputView`] handle
/// afterward via [`crate::transports::Transport::give_output`]).
pub fn bootstrap_acp_worker_runtime(
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let TargetConfiguration::Acp(acp_target) = &target_member.target else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires ACP target".to_string(),
        });
    };
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires target working directory".to_string(),
        });
    };
    initialize_persistent_acp_worker_runtime(
        target_member,
        acp_target,
        working_directory.as_path(),
        runtime_directory,
    )
}

fn initialize_persistent_acp_worker_runtime(
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    working_directory: &Path,
    runtime_directory: &Path,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let mut client = match acp.channel {
        AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return Err(AcpBootstrapError {
                    code: "runtime_startup_failed".to_string(),
                    reason: "ACP stdio target requires command".to_string(),
                });
            };
            AcpStdioClient::spawn(
                command,
                working_directory,
                &target_member
                    .environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect::<Vec<_>>(),
                true,
            )
            .map_err(|reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason,
            })?
        }
        AcpChannel::Http => {
            return Err(AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: "ACP http transport is not implemented".to_string(),
            });
        }
    };

    let initialize_result = client.initialize().map_err(|reason| AcpBootstrapError {
        code: ACP_ERROR_CODE_INITIALIZE_FAILED.to_string(),
        reason: format!("ACP initialize failed: {reason}"),
    })?;

    let capabilities = AcpCapabilities {
        load_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_session: initialize_result
            .get("agentCapabilities")
            .map(|value| {
                value
                    .get("promptSession")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        value
                            .get("promptCapabilities")
                            .is_some_and(serde_json::Value::is_object)
                    })
            })
            .unwrap_or(false),
    };

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        super::state::load_persisted_acp_session_id(runtime_directory, target_member.id.as_str())
            .map_err(|reason| AcpBootstrapError {
                code: "runtime_startup_failed".to_string(),
                reason: format!("failed to load persisted ACP session id: {reason}"),
            })?
    };

    let (lifecycle, lifecycle_session_id) =
        if let Some(configured) = target_member.coder_session_id.as_deref() {
            (AcpLifecycleSelection::LoadSession, configured.to_string())
        } else if let Some(persisted) = persisted_session_id {
            (AcpLifecycleSelection::LoadSession, persisted)
        } else {
            (AcpLifecycleSelection::NewSession, String::new())
        };

    let session_id = match lifecycle {
        AcpLifecycleSelection::LoadSession => {
            if !capabilities.load_session {
                return Err(AcpBootstrapError {
                    code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
                    reason: "ACP agent does not advertise required load capability".to_string(),
                });
            }
            client
                .load_session(lifecycle_session_id.as_str(), working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_LOAD_FAILED.to_string(),
                    reason: format!("ACP session/load failed: {reason}"),
                })?;
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => {
            client
                .new_session(working_directory)
                .map_err(|reason| AcpBootstrapError {
                    code: ACP_ERROR_CODE_SESSION_NEW_FAILED.to_string(),
                    reason: format!("ACP session/new failed: {reason}"),
                })?
        }
    };

    super::state::persist_acp_session_id(
        runtime_directory,
        target_member.id.as_str(),
        session_id.as_str(),
    )
    .map_err(|reason| AcpBootstrapError {
        code: "runtime_startup_failed".to_string(),
        reason: format!("failed to persist ACP session id: {reason}"),
    })?;

    if !capabilities.prompt_session {
        return Err(AcpBootstrapError {
            code: ACP_ERROR_CODE_MISSING_CAPABILITY.to_string(),
            reason: "ACP agent does not advertise required prompt capability".to_string(),
        });
    }

    Ok(PersistentAcpWorkerRuntime { client, session_id })
}
