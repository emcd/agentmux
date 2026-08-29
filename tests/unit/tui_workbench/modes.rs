//! Communication/Interaction mode switching: the F4 toggle, the drafts and
//! cursors each mode preserves across a round trip, the picker auto-open on
//! targetless entry, the look refresh on targeted entry, and the Interaction
//! region's swap between the raww input and the choice pane.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::{
    runtime::error::RuntimeError,
    tui::workbench::{WorkbenchField, WorkbenchMode, WorkbenchPickerColumn},
};

use super::{key_event, make_state};

#[test]
fn f4_toggles_screen_mode() {
    let mut state = make_state();
    assert_eq!(state.mode(), WorkbenchMode::Communication);
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should toggle mode");
    assert_eq!(state.mode(), WorkbenchMode::Interaction);
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should toggle mode back");
    assert_eq!(state.mode(), WorkbenchMode::Communication);
}

#[test]
fn f4_preserves_per_mode_drafts_across_switches() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::To);
    state.insert_text("user");
    state.set_focus(WorkbenchField::Message);
    state.insert_text("hello");

    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    // Entering Interaction without a target auto-opens the picker; dismiss it
    // so typing lands in the Write (raww) input rather than the picker filter.
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc should close the auto-opened picker");
    for character in "echo".chars() {
        state
            .dispatch_event(key_event(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("raww typing should be handled");
    }
    assert_eq!(state.raww_draft(), "echo");

    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch back to communication");
    assert_eq!(state.message_field(), "hello");
    assert_eq!(
        state.to_field(),
        "user",
        "the To draft must survive a mode round trip"
    );

    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction again");
    assert_eq!(state.raww_draft(), "echo");
}

#[test]
fn interaction_mode_typing_updates_raww_draft() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc should close the auto-opened picker");
    for character in "ls".chars() {
        state
            .dispatch_event(key_event(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("raww typing should be handled");
    }
    assert_eq!(state.raww_draft(), "ls");
}

#[test]
fn interaction_mode_enter_without_target_is_validation_error() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    // Close the auto-opened picker so Enter exercises the raww dispatch path,
    // which requires an active interaction target.
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc should close the auto-opened picker");
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_target")
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn f4_into_interaction_without_target_auto_opens_picker() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    assert_eq!(state.mode(), WorkbenchMode::Interaction);
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
}

#[test]
fn f4_into_interaction_with_target_does_not_auto_open_picker() {
    let mut state = make_state();
    state.set_interaction_target("master");
    // Entering Interaction with a target re-captures the look snapshot; the relay
    // is unavailable in tests, so that refresh errors, but the mode still switches
    // and — unlike the no-target case — the picker does not auto-open.
    let _ = state.dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE));
    assert_eq!(state.mode(), WorkbenchMode::Interaction);
    assert!(!state.picker_open());
}

#[test]
fn interaction_region_swaps_between_raww_and_choice_pane() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state.set_interaction_target("master");
    // The entry look refresh errors against the dead relay; the mode still
    // switches, which is all this test needs to exercise the region swap.
    let _ = state.dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE));
    assert_eq!(state.mode(), WorkbenchMode::Interaction);

    assert!(
        state.interaction_shows_raww(),
        "raww region shows when no pending requests exist"
    );

    state.inject_pending_choice("master");
    assert!(
        !state.interaction_shows_raww(),
        "choice pane replaces raww region when raww empty and pending exists"
    );

    state
        .dispatch_event(key_event(KeyCode::Char('x'), KeyModifiers::NONE))
        .expect("raww typing should be handled");
    assert!(
        state.interaction_shows_raww(),
        "raww region reclaims the region once raww input is non-empty"
    );
}

#[test]
fn write_draft_cursor_position_survives_a_mode_round_trip() {
    let mut state = make_state();
    // Enter Interaction (no target auto-opens the picker; dismiss it so typing
    // lands in the Write input) and type a draft.
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 to interaction");
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc closes the auto-opened picker");
    for character in "echo".chars() {
        state
            .dispatch_event(key_event(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("raww typing should be handled");
    }
    // Move the Write cursor into the middle of the draft: "ec|ho".
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left moves the write cursor");
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left moves the write cursor");

    // Round-trip through Communication and back.
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 to communication");
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 back to interaction");
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc closes the auto-opened picker");

    // The preserved cursor inserts mid-draft, proving both the draft and its
    // cursor index survived the round trip.
    state
        .dispatch_event(key_event(KeyCode::Char('X'), KeyModifiers::NONE))
        .expect("raww typing should be handled");
    assert_eq!(state.raww_draft(), "ecXho");
}

#[test]
fn reentering_interaction_with_a_target_refreshes_the_look_snapshot() {
    let mut state = make_state();
    state.set_interaction_target("acp");
    // Entering Interaction with an existing target re-captures the look snapshot.
    // With no relay reachable in tests the refresh surfaces the relay-unavailable
    // outcome, proving entry attempts a look instead of showing a buffer frozen
    // from a prior visit.
    let result = state.dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => assert_eq!(code, "relay_unavailable"),
        Err(RuntimeError::Io { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied)
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn entering_interaction_without_a_target_opens_picker_without_a_look_attempt() {
    let mut state = make_state();
    // With no target yet, F4 opens the picker to choose one and must not attempt
    // a look (no relay round trip, hence no error).
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 opens picker without a look attempt");
    assert!(state.picker_open());
    assert_eq!(state.mode(), WorkbenchMode::Interaction);
}
