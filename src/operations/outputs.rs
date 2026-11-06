//! Canonical operation outputs: typed success payloads.
//!
//! Outputs mirror the relay outcome shapes the adapter renders into
//! the MCP contract payloads (`schema_version` included: it is
//! relay-authored data, not presentation). Optional fields are
//! `Option` and the adapter omits absent ones rather than rendering
//! null. Relay-domain element types (`SendResult`,
//! `LookSnapshotPayload`, bundle and credential records) are reused,
//! not redefined, so the operation contract cannot drift from the
//! relay outcome it adapts.

use crate::relay::{
    BundleTransitionEntry, ListedBundle, ListedRelay, ListedSessionTransport,
    LookSnapshotPayload, PendingChoiceEntry, RelayDiagnostic, SendResult,
};

use super::inputs::DecideOutcome;

/// `send` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct SendOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Caller correlation id, present only when supplied.
    pub request_id: Option<String>,
    /// Session the request was attributed to.
    pub requester_session: String,
    /// Display name, when the relay supplies one.
    pub sender_display_name: Option<String>,
    /// Verified principal id, when the requester presented a
    /// store-backed credential.
    pub authenticated_identity: Option<String>,
    /// Cross-relay origin subject, echoed only on peer-ingress sends.
    pub on_behalf_of: Option<String>,
    /// Per-target delivery dispositions.
    pub results: Vec<SendResult>,
}

/// `look` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct LookOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Session the request was attributed to.
    pub requester_session: String,
    /// Qualified inspected target.
    pub target_session: String,
    /// Capture timestamp.
    pub captured_at: String,
    /// Verified principal id, when the requester presented a
    /// store-backed credential.
    pub authenticated_identity: Option<String>,
    /// Reserved delegated-identity subject; absent until activated.
    pub on_behalf_of: Option<String>,
    /// Snapshot payload with freshness and generation metadata.
    pub snapshot: LookSnapshotPayload,
}

/// `list.principals` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ListPrincipalsOutput {
    /// Session-list schema version for the local path.
    pub schema_version: String,
    /// Listed bundles (Down synthesis for unreachable home handled
    /// in dispatch).
    pub bundles: Vec<ListedBundle>,
    /// Origin-local relay alias, echoed on the foreign path only.
    pub relay: Option<String>,
}

/// `list.namespaces` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ListNamespacesOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Discovered namespaces.
    pub namespaces: Vec<String>,
    /// Origin-local relay alias, echoed on the foreign path only.
    pub relay: Option<String>,
}

/// `list.relays` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ListRelaysOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Configured outbound peer relays.
    pub relays: Vec<ListedRelay>,
}

/// `list.decisions` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ListDecisionsOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Pending choice requests.
    pub pending_requests: Vec<PendingChoiceEntry>,
}

/// `decide` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct DecideOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Resolution status.
    pub status: String,
    /// Resolved choice request id.
    pub choice_request_id: String,
    /// Applied outcome.
    pub outcome: DecideOutcome,
    /// Relay-derived resolving actor, when supplied.
    pub decided_by: Option<String>,
    /// Machine-readable resolution reason, when supplied.
    pub reason_code: Option<String>,
    /// Human-readable resolution reason, when supplied.
    pub reason: Option<String>,
}

/// `raww` operation outcome. Delivery is asynchronous; the terminal
/// outcome arrives out-of-band.
#[derive(Clone, Debug, PartialEq)]
pub struct RawwOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Dispatch receipt status (always `queued`).
    pub status: String,
    /// Qualified written target.
    pub target_session: String,
    /// Transport the target was reached on.
    pub transport: ListedSessionTransport,
    /// Caller correlation id, present only when supplied.
    pub request_id: Option<String>,
    /// Dispatch message id, when the relay supplies one.
    pub message_id: Option<String>,
}

/// `up` / `down` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct BundleTransitionOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Transition action taken.
    pub action: String,
    /// Per-bundle reconciliation results.
    pub bundles: Vec<BundleTransitionEntry>,
    /// Bundles that changed state.
    pub changed_bundle_count: usize,
    /// Bundles degraded.
    pub degraded_bundle_count: usize,
    /// Bundles skipped.
    pub skipped_bundle_count: usize,
    /// Bundles that failed.
    pub failed_bundle_count: usize,
    /// Whether any bundle changed.
    pub changed_any: bool,
}

/// `new.peer` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct NewPeerOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Registered principal id.
    pub principal_id: String,
    /// Principal kind.
    pub principal_type: String,
    /// Minted PSK, present only in `Response` destination mode.
    pub psk: Option<String>,
    /// File the PSK was written to, when a file sink was used.
    pub written_path: Option<String>,
    /// Config snippet for the new principal.
    pub config_snippet: String,
    /// Registration advisories; empty when the relay raised none.
    pub diagnostics: Vec<RelayDiagnostic>,
}

/// `change.psk` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangePskOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Principal id whose credential rotated.
    pub principal_id: String,
    /// Rotated PSK, present only in `Response` destination mode.
    pub psk: Option<String>,
    /// File the PSK was written to, when a file sink was used.
    pub written_path: Option<String>,
}

/// `change.scope` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeScopeOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Peer relay principal id whose grant was replaced.
    pub principal_id: String,
    /// Canonical applied scope.
    pub scope: String,
    /// Update advisories; empty when the relay raised none.
    pub diagnostics: Vec<RelayDiagnostic>,
}

/// `drop.peer` operation outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPeerOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Deleted principal id.
    pub principal_id: String,
    /// Principal kind.
    pub principal_type: String,
    /// Relay-owned canonical credential file, session principals
    /// only.
    pub credential_path: Option<String>,
}

/// `link.peer` operation outcome. The PSK never appears here.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkPeerOutput {
    /// Relay schema version echoed from the outcome.
    pub schema_version: String,
    /// Local peer alias whose slot received the credential.
    pub alias: String,
    /// File the credential was written to.
    pub written_path: String,
}
