//! To-field recipient completion: candidate pooling across the active and
//! cross-bundle sources, cycling, and the accept/retrigger boundary.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{key_event, make_state};

#[test]
fn completion_navigation_in_to_field_uses_up_and_down() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "agent", "relay"]);
    state.insert_text("@a");
    assert_eq!(state.to_field(), "agent");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should be handled");
    assert_eq!(state.to_field(), "alpha");
    state
        .dispatch_event(key_event(KeyCode::Up, KeyModifiers::NONE))
        .expect("up should be handled");
    assert_eq!(state.to_field(), "agent");
}

#[test]
fn to_completion_merges_active_and_cross_bundle_candidates() {
    let mut state = make_state();
    state.set_recipients(&["alpha@agentmux"]);
    state.set_cross_bundle_candidates(&["alto@secondary", "bravo@secondary"]);
    // `@a` matches one active-bundle and one cross-bundle candidate; the merged,
    // sorted pool offers both and Down cycles from the first to the second.
    state.insert_text("@a");
    assert_eq!(state.to_field(), "alpha@agentmux");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should cycle to the cross-bundle candidate");
    assert_eq!(state.to_field(), "alto@secondary");
}

#[test]
fn to_completion_offers_cross_bundle_only_prefix() {
    let mut state = make_state();
    state.set_recipients(&["alpha@agentmux"]);
    state.set_cross_bundle_candidates(&["bravo@secondary"]);
    // A prefix no active-bundle recipient can satisfy still completes from the
    // relay-wide cross-bundle source to the full session@bundle principal id.
    state.insert_text("@b");
    assert_eq!(state.to_field(), "bravo@secondary");
}

#[test]
fn completion_navigation_stops_after_accept_until_retriggered() {
    let mut state = make_state();
    state.set_recipients(&["master", "mcp"]);
    state.insert_text("m");
    state
        .dispatch_event(key_event(KeyCode::Char(' '), KeyModifiers::CONTROL))
        .expect("ctrl+space should be handled");
    assert_eq!(state.to_field(), "master");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should accept completion");
    assert_eq!(state.to_field(), "master, ");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should be handled");
    assert_eq!(state.to_field(), "master, ");
    state.insert_text("m");
    state
        .dispatch_event(key_event(KeyCode::Char(' '), KeyModifiers::CONTROL))
        .expect("ctrl+space should retrigger completion mode");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should cycle in active completion mode");
    assert_eq!(state.to_field(), "master, mcp");
}
