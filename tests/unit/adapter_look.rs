use agentmux::adapter::look::{
    Freshness, PrimeWait, Snapshot, SnapshotEntry, SnapshotSource,
    fence_snapshot, snapshot_result,
};

/// A zero timeout means no wait: the first poll answers stale.
#[test]
fn zero_timeout_never_waits() {
    let wait = PrimeWait::new(1_000, 0);
    assert!(wait.is_expired(1_000));
}

/// A nonzero prime-wait expires exactly at now plus timeout.
#[test]
fn prime_wait_expires_at_deadline() {
    let wait = PrimeWait::new(1_000, 500);
    assert!(!wait.is_expired(1_499));
    assert!(wait.is_expired(1_500));
}

/// Snapshot results carry full freshness metadata.
#[test]
fn snapshot_carries_freshness_metadata() {
    let entries = vec![SnapshotEntry {
        kind: "text".to_string(),
        seq: 3,
        text_base64: "aGk=".to_string(),
        truncated: false,
    }];
    let result = snapshot_result(&Snapshot {
        correlation_id: "c-1".to_string(),
        adapter_generation: 2,
        entries,
        freshness: Freshness::Fresh,
        source: SnapshotSource::Live,
        stale: false,
        discontinuity: false,
        stale_reason_code: None,
        entries_total_override: None,
    });
    assert_eq!(result["correlation_id"], "c-1");
    assert_eq!(result["freshness"], "fresh");
    assert_eq!(result["stale"], false);
    assert_eq!(result["entries_total"], 1);
    assert_eq!(result["entries"][0]["seq"], 3);
}

/// Fresh snapshots render fresh with stale false; stale snapshots
/// render stale with the coded reason.
#[test]
fn snapshot_freshness_renders_verbatim() {
    let fresh = snapshot_result(&Snapshot {
        correlation_id: "c-1".to_string(),
        adapter_generation: 2,
        entries: Vec::new(),
        freshness: Freshness::Fresh,
        source: SnapshotSource::Live,
        stale: false,
        discontinuity: false,
        entries_total_override: None,
        stale_reason_code: None,
    });
    assert_eq!(fresh["freshness"], "fresh");
    assert_eq!(fresh["stale"], false);
    let stale = snapshot_result(&Snapshot {
        correlation_id: "c-2".to_string(),
        adapter_generation: 2,
        entries: Vec::new(),
        freshness: Freshness::Stale,
        source: SnapshotSource::Synthetic,
        stale: true,
        entries_total_override: None,
        discontinuity: true,
        stale_reason_code: Some("priming".to_string()),
    });
    assert_eq!(stale["freshness"], "stale");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["stale_reason_code"], "priming");
}

/// The fence snapshot is synthetic, stale, empty, and discontinuous.
#[test]
fn fence_snapshot_is_empty_and_stale() {
    let result = fence_snapshot("c-9", 4);
    assert_eq!(result["entries_total"], 0);
    assert_eq!(result["returned_entries_count"], 0);
    assert_eq!(result["stale"], true);
    assert_eq!(result["discontinuity"], true);
    assert_eq!(result["stale_reason_code"], "fence_terminalized");
}
