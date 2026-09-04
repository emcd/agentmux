//! Timestamp and stream-event helpers for the async-delivery worker.
//!
//! - `now_rfc3339` formats the current UTC time as RFC 3339. Used by the
//!   envelope builders to stamp `created_at` on every emitted `RelayStreamEvent`,
//!   and by the worker's intake to stamp an entry's payload exactly once.
//! - `acp_respawn_stream_event` builds the canonical `RelayStreamEvent` template
//!   the ACP worker broadcasts to watching hosts on respawn.
//!
//! The outcome-mapping and collect-site functions that used to live here went
//! with the push model. Nothing is handed to a transport any more, so there is no
//! resolved future to join and no transport-side outcome to translate: a member's
//! outcome is settled where its unit is acknowledged, and reported from the
//! entry the mailbox held.
//!
//! (`stream_send_to_broadcast_status` lives in `envelope.rs` because every
//! call site is inside an envelope builder.)

use crate::relay::{identity::canonical_session_id, stream::RelayStreamEvent};
use time::format_description::well_known::Rfc3339;

pub(super) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Builds the canonical `RelayStreamEvent` template the ACP worker broadcasts
/// to watching hosts on respawn. The `target_session` field is rewritten per
/// recipient by the broadcast_ui closure in `build_acp_driver_services`.
pub(super) fn acp_respawn_stream_event(
    event_type: &str,
    namespace: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        target_session: canonical_session_id(target_session, namespace),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload,
    }
}
