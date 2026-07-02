//! Unit coverage for the shared per-session startup-failure fold.
//!
//! Both the relay-host autostart summary and the bundle-watcher load/reload path
//! surface why a bundle produced no ready session by folding its per-session
//! failures through [`fold_startup_failures`]. These tests lock the joined reason
//! wording and the structured `failed_sessions` detail shape so the two surfaces
//! cannot silently drift apart again.

use agentmux::relay::{
    FoldedStartupFailures, ListedSessionTransport, StartupFailureRecord, fold_startup_failures,
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
    assert_eq!(fold_startup_failures(&[]), None);
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

    let folded = fold_startup_failures(&failures).expect("non-empty failures fold to Some");

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
