use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{configuration::SessionType, transports::StructuredEntry};
// Delivery/look vocabulary now lives canonically in `crate::transports`. Re-export
// only the variants that relay contract wire types below embed, so external
// deserializers of those payloads resolve them from `crate::relay`: `SendOutcome`
// rides in `SendResult`; `LookFreshness`/`LookSnapshotSource` ride in
// `LookSnapshotPayload`. `DeliveryPayloadMode` is not embedded in a wire type but is
// re-exported as the convenience path for relay's own delivery dispatch, which
// branches on it across the worker/handler surface. Relay-internal code that merely
// consumes a transport-vocabulary type without embedding it (e.g. the
// `read_worker_readiness` stringify over `WorkerReadinessState`) imports it from
// `crate::transports` directly rather than through this re-export.
pub use crate::transports::vocabulary::{
    DeliveryPayloadMode, LookFreshness, LookSnapshotSource, SendOutcome,
};

/// Declared session type for one listed bundle session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedSessionTransport {
    Tmux,
    Acp,
    Pty,
    Ui,
    Pubsub,
}

impl From<SessionType> for ListedSessionTransport {
    fn from(value: SessionType) -> Self {
        match value {
            SessionType::Tmux => Self::Tmux,
            SessionType::Acp => Self::Acp,
            SessionType::Pty => Self::Pty,
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

/// Snapshot payload variant for look responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "snapshot_format", rename_all = "snake_case")]
pub enum LookSnapshotPayload {
    Lines {
        snapshot_lines: Vec<String>,
    },
    StructuredEntriesV1 {
        snapshot_entries: Vec<StructuredEntry>,
        /// Total entries available in the replay buffer before tail/offset
        /// windowing. Lets callers detect truncation and bound backward walks.
        entries_total: usize,
        /// Count of entries actually returned in `snapshot_entries` after
        /// applying the tail-N window and `offset`.
        returned_entries_count: usize,
        freshness: LookFreshness,
        snapshot_source: LookSnapshotSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        stale_reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot_age_ms: Option<u64>,
    },
}

/// One persisted startup-failure record surfaced in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StartupFailureRecord {
    pub session_id: String,
    pub transport: ListedSessionTransport,
    pub code: String,
    pub reason: String,
    pub timestamp: String,
    pub sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// A folded operator-facing reason plus structured detail for a set of
/// per-session startup failures.
///
/// Both the relay-host autostart summary and the bundle-watcher load/reload path
/// give up on a bundle when no configured session reaches ready state. Rather than
/// each surface independently re-deriving (and drifting on) the "why", they share
/// this fold: [`reason`](Self::reason) names each failed session and its cause
/// inline, and [`details`](Self::details) carries the structured per-session
/// records under a `failed_sessions` key.
#[derive(Clone, Debug, PartialEq)]
pub struct FoldedStartupFailures {
    /// Human-readable summary naming each failed session and its cause.
    pub reason: String,
    /// Structured `{ "failed_sessions": [ ... ] }` detail object.
    pub details: Value,
}

/// Folds per-session startup failures into a shared operator-facing reason and
/// structured detail, or `None` when there are no recorded failures (for example a
/// bundle that configures no sessions) — the caller then keeps its plain "nothing
/// became ready" placeholder rather than inventing a per-session breakdown.
pub fn fold_startup_failures(
    failed_startups: &[StartupFailureRecord],
) -> Option<FoldedStartupFailures> {
    if failed_startups.is_empty() {
        return None;
    }
    let joined = failed_startups
        .iter()
        .map(|failure| format!("{}: {}", failure.session_id, failure.reason))
        .collect::<Vec<_>>()
        .join("; ");
    let failed_sessions = failed_startups
        .iter()
        .map(|failure| {
            json!({
                "session_id": failure.session_id,
                "transport": failure.transport,
                "code": failure.code,
                "reason": failure.reason,
                "details": failure.details,
            })
        })
        .collect::<Vec<_>>();
    Some(FoldedStartupFailures {
        reason: format!(
            "no configured session reached ready state ({} failed) -- {joined}",
            failed_startups.len()
        ),
        details: json!({ "failed_sessions": failed_sessions }),
    })
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
    pub principals: Vec<ListedSession>,
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

/// Structured relay error object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl RelayError {
    /// Renders an operator-facing message that folds the configuration
    /// diagnostic detail (`path` + `cause`) a config parse / unknown-field
    /// failure carries into the summary, so a CLI surface such as `agentmux up`
    /// names the offending file and the serde-reported field instead of a bare
    /// summary like "bundle configuration is invalid".
    ///
    /// Only errors whose `details` carry both string `path` and `cause` keys —
    /// the shape every configuration load failure emits — are augmented; any
    /// other error returns its plain `message`. The full structured `details`
    /// payload is left untouched for the inscription stream regardless.
    #[must_use]
    pub fn operator_message(&self) -> String {
        let Some(details) = self.details.as_ref() else {
            return self.message.clone();
        };
        match (
            details.get("path").and_then(Value::as_str),
            details.get("cause").and_then(Value::as_str),
        ) {
            (Some(path), Some(cause)) => format!("{} ({path}: {cause})", self.message),
            _ => self.message.clone(),
        }
    }
}

/// Relay-pushed stream event payload. The addressee's bundle is carried in the
/// canonical `target_session` suffix (`session@bundle`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayStreamEvent {
    pub event_type: String,
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
        requester_session: Option<String>,
    },
    Send {
        request_id: Option<String>,
        requester_session: String,
        message: String,
        targets: Vec<String>,
        broadcast: bool,
        #[serde(default)]
        quiet_window_ms: Option<u64>,
        /// Origin principal this request is forwarded on behalf of. Set only by a
        /// cross-relay forward (the origin requester's `principal_id`); absent
        /// otherwise. Advisory and opaque — the receiving relay honors it only
        /// from a relay-principal (peer) requester and carries it, uninterpreted,
        /// into the delivered envelope; it is never an authorization input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<String>,
    },
    Look {
        requester_session: String,
        target_session: String,
        #[serde(default)]
        lines: Option<usize>,
        /// Entries to skip from the newest end before applying the tail-N
        /// window. Enables backward walking of the ACP replay buffer for older
        /// context. Ignored for tmux targets, which capture pane scrollback.
        #[serde(default)]
        offset: Option<usize>,
    },
    Raww {
        request_id: Option<String>,
        requester_session: String,
        target_session: String,
        text: String,
        #[serde(default)]
        no_enter: bool,
        /// Origin principal this request is forwarded on behalf of. Set only by a
        /// cross-relay forward; absent otherwise. Raw input has no delivered
        /// attribution envelope, so the receiving relay does not surface it — the
        /// field carries the origin on the wire for symmetry with `Send` and
        /// future raww attribution/observability.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<String>,
    },
    ChoicesPick {
        choice_request_id: String,
        outcome: String,
        #[serde(default)]
        option_id: Option<String>,
    },
    ChoicesList,
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
    IdentityIntrospect {
        target_session: String,
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
        request_id: Option<String>,
        requester_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_display_name: Option<String>,
        /// Verified `principal_id` of the requester, present only when the
        /// requester's connection presented a store-backed credential; absent for
        /// socket-trust sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authenticated_identity: Option<String>,
        /// Advisory origin subject the requester was forwarded on behalf of.
        /// Echoed on a cross-relay ingress `Send` (the peer-supplied origin
        /// `principal_id`, honored only from a relay-principal requester); absent
        /// for a locally-originated send. Never an authorization input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<String>,
        results: Vec<SendResult>,
    },
    Look {
        schema_version: String,
        requester_session: String,
        target_session: String,
        captured_at: String,
        /// Verified `principal_id` of the requester, present only when the
        /// requester's connection presented a store-backed credential; absent
        /// for socket-trust sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authenticated_identity: Option<String>,
        /// Delegated principal the requester is acting on behalf of. Reserved
        /// for a future delegated-identity delta. Defined here so consumers can
        /// handle it without a breaking change when it is activated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<String>,
        #[serde(flatten)]
        snapshot: LookSnapshotPayload,
    },
    Raww {
        schema_version: String,
        /// Always `queued`: raww delivery is asynchronous. The terminal
        /// delivery outcome is reported out-of-band via `delivery_outcome`
        /// stream events, not in this immediate response.
        status: String,
        target_session: String,
        transport: ListedSessionTransport,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    ChoicesPick {
        schema_version: String,
        status: String,
        choice_request_id: String,
        outcome: String,
        /// Relay-derived, association-bound actor that resolved the choice.
        /// Mirrors the `decided_by` field on the `choices.resolved` event.
        #[serde(skip_serializing_if = "Option::is_none")]
        decided_by: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ChoicesList {
        schema_version: String,
        pending_requests: Vec<PendingChoiceEntry>,
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
    IdentityIntrospect {
        schema_version: String,
        /// Verified `principal_id` of the introspected session.
        principal_id: String,
        /// Expiry of the introspected principal's credential (ISO 8601).
        /// Present only when the principal has a bounded expiry; absent for
        /// principals that never expire, rather than a placeholder timestamp.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        /// Delegated principal the introspected session is acting on behalf of.
        /// Reserved: the setting mechanism lands in a later delta, so this is
        /// always absent for now. Defined here so consumers can handle it
        /// without a breaking change when it is activated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<String>,
        /// True when the introspected session presented a store-backed
        /// credential; false for socket-trust sessions.
        verified: bool,
    },
    Error {
        error: RelayError,
    },
}

/// One pending choice request entry returned by `RelayResponse::ChoicesList`.
///
/// Field set mirrors the `choices.requested` stream event payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PendingChoiceEntry {
    pub message_id: String,
    pub choice_request_id: String,
    pub target_session: String,
    pub requested_kind: String,
    pub requested_details: Value,
    pub enqueued_at: String,
}
