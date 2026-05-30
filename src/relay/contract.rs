use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{acp::AcpSnapshotEntry, configuration::SessionType};

/// Declared session type for one listed bundle session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedSessionTransport {
    Tmux,
    Acp,
    Ui,
    Pubsub,
}

impl From<SessionType> for ListedSessionTransport {
    fn from(value: SessionType) -> Self {
        match value {
            SessionType::Tmux => Self::Tmux,
            SessionType::Acp => Self::Acp,
            SessionType::Ui => Self::Ui,
            SessionType::Pubsub => Self::Pubsub,
        }
    }
}

/// One configured session entry in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ListedSession {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub transport: ListedSessionTransport,
    pub ready: bool,
}

/// Bundle live state in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedBundleState {
    Up,
    Down,
}

/// Startup health marker for an `up` bundle in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedBundleStartupHealth {
    Healthy,
    Degraded,
}

/// Freshness status for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookFreshness {
    Fresh,
    Stale,
}

/// Source marker for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookSnapshotSource {
    LiveBuffer,
    None,
}

/// Snapshot payload variant for look responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "snapshot_format", rename_all = "snake_case")]
pub enum LookSnapshotPayload {
    Lines {
        snapshot_lines: Vec<String>,
    },
    AcpEntriesV1 {
        snapshot_entries: Vec<AcpSnapshotEntry>,
        freshness: AcpLookFreshness,
        snapshot_source: AcpLookSnapshotSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        stale_reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot_age_ms: Option<u64>,
    },
}

/// One persisted startup-failure record surfaced in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StartupFailureRecord {
    pub bundle_name: String,
    pub session_id: String,
    pub transport: ListedSessionTransport,
    pub code: String,
    pub reason: String,
    pub timestamp: String,
    pub sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Canonical listed bundle payload for session-listing responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ListedBundle {
    pub id: String,
    pub hosted: bool,
    pub state: ListedBundleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_health: Option<ListedBundleStartupHealth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    pub startup_failure_count: usize,
    pub recent_startup_failures: Vec<StartupFailureRecord>,
    pub sessions: Vec<ListedSession>,
}

/// Per-target delivery result for one `send` request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SendResult {
    pub target_session: String,
    pub message_id: String,
    pub outcome: SendOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Reconciliation results for one bundle reconciliation pass.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub bootstrap_session: Option<String>,
    pub created_sessions: Vec<String>,
    pub pruned_sessions: Vec<String>,
}

/// Managed-session cleanup results for relay shutdown.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShutdownReport {
    pub pruned_sessions: Vec<String>,
    pub killed_tmux_server: bool,
}

/// Per-bundle startup pass outcome for relay host autostart.
#[derive(Clone, Debug, PartialEq)]
pub struct BundleStartupReport {
    pub ready_session_count: usize,
    pub failed_startups: Vec<StartupFailureRecord>,
}

/// Per-target delivery outcome for `send`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SendOutcome {
    Queued,
    Delivered,
    Timeout,
    DroppedOnShutdown,
    Failed,
}

/// Payload handling mode for one async delivery task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPayloadMode {
    EnvelopeMessage,
    RawInput,
}

/// Structured relay error object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Relay-pushed stream event payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayStreamEvent {
    pub event_type: String,
    pub bundle_name: String,
    pub target_session: String,
    pub created_at: String,
    pub payload: Value,
}

/// Per-bundle transition result for `up`/`down`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleTransitionEntry {
    pub bundle_name: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Relay request protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RelayRequest {
    Up,
    Down,
    List {
        sender_session: Option<String>,
    },
    Send {
        request_id: Option<String>,
        sender_session: String,
        message: String,
        targets: Vec<String>,
        broadcast: bool,
        #[serde(default)]
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        quiescence_timeout_ms: Option<u64>,
        #[serde(default)]
        acp_turn_timeout_ms: Option<u64>,
    },
    Look {
        requester_session: String,
        target_session: String,
        #[serde(default)]
        lines: Option<usize>,
        #[serde(default)]
        bundle_name: Option<String>,
    },
    Raww {
        request_id: Option<String>,
        sender_session: String,
        target_session: String,
        text: String,
        #[serde(default)]
        no_enter: bool,
        #[serde(default)]
        bundle_name: Option<String>,
    },
    PermissionResolve {
        permission_request_id: String,
        outcome: String,
        #[serde(default)]
        option_id: Option<String>,
        #[serde(default)]
        bundle_name: Option<String>,
        #[serde(default)]
        ui_session_id: Option<String>,
    },
    PermissionList {
        #[serde(default)]
        bundle_name: Option<String>,
    },
    NewPeer {
        principal_id: String,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        output_path: Option<String>,
    },
    ChangePsk {
        principal_id: String,
    },
}

/// Relay response protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayResponse {
    BundleTransition {
        schema_version: String,
        action: String,
        bundles: Vec<BundleTransitionEntry>,
        changed_bundle_count: usize,
        skipped_bundle_count: usize,
        failed_bundle_count: usize,
        changed_any: bool,
    },
    List {
        schema_version: String,
        bundle: ListedBundle,
    },
    Send {
        schema_version: String,
        bundle_name: String,
        request_id: Option<String>,
        sender_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_display_name: Option<String>,
        results: Vec<SendResult>,
    },
    Look {
        schema_version: String,
        bundle_name: String,
        requester_session: String,
        target_session: String,
        captured_at: String,
        #[serde(flatten)]
        snapshot: LookSnapshotPayload,
    },
    Raww {
        schema_version: String,
        status: String,
        target_session: String,
        transport: ListedSessionTransport,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    PermissionDecision {
        schema_version: String,
        status: String,
        permission_request_id: String,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    PermissionList {
        schema_version: String,
        bundle_name: String,
        pending_requests: Vec<PendingPermissionEntry>,
    },
    NewPeer {
        schema_version: String,
        principal_id: String,
        principal_type: String,
        /// Raw PSK; omitted when the credential was written to `output_path`.
        #[serde(skip_serializing_if = "Option::is_none")]
        psk: Option<String>,
        /// Absolute path the PSK was written to; present only with `--output`.
        #[serde(skip_serializing_if = "Option::is_none")]
        output_path: Option<String>,
        config_snippet: String,
    },
    ChangePsk {
        schema_version: String,
        principal_id: String,
        psk: String,
    },
    Error {
        error: RelayError,
    },
}

/// One pending permission request entry returned by `RelayResponse::PermissionList`.
///
/// Field set mirrors the `permission.requested` stream event payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PendingPermissionEntry {
    pub message_id: String,
    pub permission_request_id: String,
    pub target_session: String,
    pub requested_kind: String,
    pub requested_details: Value,
    pub enqueued_at: String,
}
