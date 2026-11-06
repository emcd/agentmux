use agentmux::adapter::binding::{
    Binding, CallKind, Direction, PendingTable, PinnedLaunch,
    establish_binding,
};

/// Minted ids start at 1 and advance monotonically.
#[test]
fn wire_ids_start_at_one_and_advance() {
    let mut table = PendingTable::new();
    let first = table
        .mint(CallKind::RequestChoice, None)
        .expect("mint succeeds");
    let second = table.mint(CallKind::Look, None).expect("mint succeeds");
    assert_eq!((first, second), (1, 2));
}

/// Direction spaces are independent: the same numeric id in each
/// direction names a different open entry.
#[test]
fn direction_spaces_are_independent() {
    let mut table = PendingTable::new();
    let outbound = table.mint(CallKind::Submit, None).expect("mint succeeds");
    table
        .note_inbound(1, CallKind::Look, None)
        .expect("inbound opens");
    assert_eq!(outbound, 1);
    assert!(table.admit(Direction::Outbound, 1).is_some());
    assert!(table.admit(Direction::Inbound, 1).is_some());
}

/// Admission follows the open entry; unknown ids admit nothing and
/// provoke no wire error by themselves.
#[test]
fn unknown_wire_id_admits_nothing() {
    let table = PendingTable::new();
    assert!(table.admit(Direction::Outbound, 9).is_none());
}

/// A retired entry no longer admits replies: duplicates resolve to a
/// local diagnostic, never a second disposition.
#[test]
fn retired_entry_admits_no_reply() {
    let mut table = PendingTable::new();
    table
        .mint(CallKind::Persist, Some("c-1".into()))
        .expect("mint succeeds");
    let retired = table.retire(Direction::Outbound, 1);
    assert!(retired.is_some());
    assert!(table.admit(Direction::Outbound, 1).is_none());
}

/// Clearing returns every entry for per-type terminalization and
/// keeps the counters, so post-cleanup calls never reuse wire ids.
#[test]
fn clear_returns_entries_without_reusing_ids() {
    let mut table = PendingTable::new();
    table.mint(CallKind::Submit, None).expect("mint succeeds");
    table
        .note_inbound(5, CallKind::Ping, None)
        .expect("inbound opens");
    let cleared = table.clear();
    assert_eq!(cleared.len(), 2);
    assert!(table.is_empty());
    let next = table
        .mint(CallKind::RequestChoice, None)
        .expect("mint succeeds");
    assert_eq!(next, 2);
}

/// A regressed or repeated inbound id is refused even after the
/// original entry retired or the table cleared: the high-water mark
/// survives both.
#[test]
fn inbound_reuse_is_refused_after_retire_and_clear() {
    let mut table = PendingTable::new();
    table
        .note_inbound(7, CallKind::Look, None)
        .expect("inbound opens");
    table.retire(Direction::Inbound, 7);
    assert!(table.note_inbound(7, CallKind::Look, None).is_err());
    assert!(table.note_inbound(3, CallKind::Look, None).is_err());
    table.clear();
    assert!(table.note_inbound(7, CallKind::Look, None).is_err());
    table
        .note_inbound(8, CallKind::Look, None)
        .expect("greater id opens");
}

/// Establishment carries the pinned target into the captured binding:
/// the result assigns runtime and generation, never identity.
#[test]
fn establishment_uses_pinned_target() {
    let pinned = PinnedLaunch {
        target_session: "seat-1".to_string(),
        adapter_profile: "generic".to_string(),
        adapter_executable: "agentmux-adapter".to_string(),
    };
    let (binding, _) = establish_binding(&pinned, "rt-9".to_string(), 3);
    assert_eq!(
        binding,
        Binding {
            runtime_instance: "rt-9".to_string(),
            target_session: "seat-1".to_string(),
            adapter_generation: 3,
        }
    );
}
