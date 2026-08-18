//! Unit coverage for the persisted startup-failure history and the clearing a
//! successful serve performs.
//!
//! What these guard is a *stale* record rather than a missing one. A record
//! that survives its session's recovery is a failure reported for a session
//! that is serving: it inflates `startup_failure_count` and puts an entry in
//! `recent_startup_failures`, which the TUI renders as its own line.
//!
//! It does *not* reach `startup_health`, which is derived from live readiness
//! and never reads this history. The damage is a diagnostic list carrying
//! failures that no longer apply — which is how a diagnostic list stops being
//! believed — rather than a healthy bundle reported as degraded.

use std::path::Path;

use agentmux::relay::{
    ListedSessionTransport, StartupFailureRecord, append_startup_failure, load_startup_failures,
    note_session_served_successfully,
};
use tempfile::TempDir;

fn failure(session_id: &str, reason: &str) -> StartupFailureRecord {
    StartupFailureRecord {
        session_id: session_id.to_string(),
        transport: ListedSessionTransport::Acp,
        code: "runtime_startup_failed".to_string(),
        reason: reason.to_string(),
        timestamp: "1970-01-01T00:00:00Z".to_string(),
        sequence: 0,
        details: None,
    }
}

/// The reasons still on record for one session, in history order.
fn recorded_reasons(runtime_directory: &Path, session_id: &str) -> Vec<String> {
    load_startup_failures(runtime_directory)
        .expect("load startup failures")
        .into_iter()
        .filter(|record| record.session_id == session_id)
        .map(|record| record.reason)
        .collect()
}

#[test]
fn a_successful_serve_clears_the_failure_that_preceded_it() {
    // The single cycle, which already worked. It is here so the two-cycle test
    // below cannot be satisfied by clearing having broken outright.
    let temporary = TempDir::new().expect("temporary");
    let runtime_directory = temporary.path();
    append_startup_failure(runtime_directory, failure("alpha", "first failure"))
        .expect("append first failure");
    assert_eq!(
        recorded_reasons(runtime_directory, "alpha"),
        ["first failure"]
    );

    note_session_served_successfully(runtime_directory, "alpha").expect("note first recovery");

    assert!(
        recorded_reasons(runtime_directory, "alpha").is_empty(),
        "a recovered session must leave no failure record behind"
    );
}

#[test]
fn a_second_failure_and_recovery_clears_the_second_record_too() {
    // The regression. A per-process dedup set that recorded "this session has
    // been cleared once" rather than "this session's history is empty" made the
    // second recovery a no-op, stranding the second failure until the relay
    // process restarted. Both cycles run against one runtime directory in one
    // process, which is what makes the second call hit the cache at all.
    let temporary = TempDir::new().expect("temporary");
    let runtime_directory = temporary.path();

    append_startup_failure(runtime_directory, failure("alpha", "first failure"))
        .expect("append first failure");
    note_session_served_successfully(runtime_directory, "alpha").expect("note first recovery");
    append_startup_failure(runtime_directory, failure("alpha", "second failure"))
        .expect("append second failure");
    assert_eq!(
        recorded_reasons(runtime_directory, "alpha"),
        ["second failure"],
        "the second failure must be recorded after the first cycle cleared"
    );

    note_session_served_successfully(runtime_directory, "alpha").expect("note second recovery");

    assert!(
        recorded_reasons(runtime_directory, "alpha").is_empty(),
        "the second recovery must clear the second failure, not be deduplicated away"
    );
}

#[test]
fn clearing_one_session_leaves_another_session_untouched() {
    // The eviction is keyed per session, so a busy session recovering must not
    // silently absolve a different one that is still failing.
    let temporary = TempDir::new().expect("temporary");
    let runtime_directory = temporary.path();
    append_startup_failure(runtime_directory, failure("alpha", "alpha failure"))
        .expect("append alpha failure");
    append_startup_failure(runtime_directory, failure("bravo", "bravo failure"))
        .expect("append bravo failure");

    note_session_served_successfully(runtime_directory, "alpha").expect("note alpha recovery");

    assert!(recorded_reasons(runtime_directory, "alpha").is_empty());
    assert_eq!(
        recorded_reasons(runtime_directory, "bravo"),
        ["bravo failure"],
        "one session recovering must not clear another's history"
    );
}
