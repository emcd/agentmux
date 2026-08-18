//! Unit coverage for the shared per-session startup-failure fold.
//!
//! The relay-host autostart summary, the bundle-watcher load/reload path, and the
//! `up` transition all surface why a bundle's sessions failed by folding their
//! per-session failures through [`fold_startup_failures`]. These tests lock the
//! joined reason wording and the structured `failed_sessions` detail shape so the
//! surfaces cannot silently drift apart again, and lock that a partial startup and
//! a total one differ only in the lead phrase.

use agentmux::relay::{
    FoldedStartupFailures, ListedSessionTransport, NO_READY_SESSION_LEAD, PARTIAL_STARTUP_LEAD,
    StartupFailureRecord, fold_startup_failures,
};
use serde_json::json;

fn failure(session_id: &str, code: &str, reason: &str) -> StartupFailureRecord {
    StartupFailureRecord {
        session_id: session_id.to_string(),
        transport: ListedSessionTransport::Acp,
        code: code.to_string(),
        reason: reason.to_string(),
        timestamp: "1970-01-01T00:00:00Z".to_string(),
        sequence: 0,
        details: None,
    }
}

#[test]
fn folds_empty_failures_to_none() {
    assert_eq!(fold_startup_failures(NO_READY_SESSION_LEAD, &[]), None);
    assert_eq!(fold_startup_failures(PARTIAL_STARTUP_LEAD, &[]), None);
}

#[test]
fn folds_failures_into_shared_reason_and_details() {
    let failures = vec![
        failure(
            "bravo",
            "runtime_startup_failed",
            "spawn ACP stdio command failed",
        ),
        failure(
            "charlie",
            "runtime_acp_initialize_failed",
            "initialize handshake failed",
        ),
    ];

    let folded = fold_startup_failures(NO_READY_SESSION_LEAD, &failures)
        .expect("non-empty failures fold to Some");

    assert_eq!(
        folded,
        FoldedStartupFailures {
            reason: "no configured session reached ready state (2 failed) -- \
                     bravo: spawn ACP stdio command failed; \
                     charlie: initialize handshake failed"
                .to_string(),
            details: json!({
                "failed_sessions": [
                    {
                        "session_id": "bravo",
                        "transport": ListedSessionTransport::Acp,
                        "code": "runtime_startup_failed",
                        "reason": "spawn ACP stdio command failed",
                        "details": null,
                    },
                    {
                        "session_id": "charlie",
                        "transport": ListedSessionTransport::Acp,
                        "code": "runtime_acp_initialize_failed",
                        "reason": "initialize handshake failed",
                        "details": null,
                    },
                ],
            }),
        }
    );
}

#[test]
fn folds_a_partial_startup_under_its_own_lead_phrase() {
    let failures = vec![failure(
        "bravo",
        "runtime_startup_failed",
        "spawn ACP stdio command failed",
    )];

    let partial = fold_startup_failures(PARTIAL_STARTUP_LEAD, &failures)
        .expect("non-empty failures fold to Some");
    let total = fold_startup_failures(NO_READY_SESSION_LEAD, &failures)
        .expect("non-empty failures fold to Some");

    assert_eq!(
        partial.reason,
        "bundle came up with failed sessions (1 failed) -- bravo: spawn ACP stdio command failed"
    );
    // A partial startup must not claim nothing became ready; only the lead
    // differs, so the structured detail the three surfaces render stays identical.
    assert_ne!(partial.reason, total.reason);
    assert_eq!(partial.details, total.details);
}
