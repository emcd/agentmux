//! The picker overlay's session column: what Enter does in each mode, the
//! column-scoped filter, focus switching, and how the selection is restored
//! across close/reopen and relay-driven recipient refreshes.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::{
    runtime::error::RuntimeError,
    tui::workbench::{WorkbenchField, WorkbenchMode, WorkbenchPickerColumn},
};

use super::{key_event, make_state};

#[test]
fn picker_enter_in_communication_mode_inserts_selected_recipient_into_to() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter in communication mode should insert recipient");
    assert_eq!(state.mode(), WorkbenchMode::Communication);
    assert_eq!(state.to_field(), "master");
    assert!(!state.picker_open());
}

#[test]
fn picker_enter_inserts_recipient_when_composing_the_message_body() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    // The picker is the recipient affordance regardless of which compose field
    // holds focus: opening it from the message body must still deliver the
    // recipient rather than dead-ending the operator.
    state.set_focus(WorkbenchField::Message);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should insert the recipient from message focus");
    assert_eq!(state.to_field(), "master");
    assert!(!state.picker_open());
    assert_eq!(
        state.focus(),
        WorkbenchField::Message,
        "insertion must leave compose focus where the operator left it",
    );

    // The unfocused To cursor must still track the value the picker wrote.
    // A stale index survives as a mid-field insertion point: the operator tabs
    // over, types, and lands their text inside a recipient they never edited.
    assert_eq!(
        state.to_cursor_column(),
        "master".chars().count(),
        "To cursor must sit at the end of the inserted recipient",
    );
    state.set_focus(WorkbenchField::To);
    state
        .dispatch_event(key_event(KeyCode::Char('x'), KeyModifiers::NONE))
        .expect("typing in the To field should be handled");
    assert_eq!(
        state.to_field(),
        "masterx",
        "typing after the insertion must append, not split the recipient",
    );
}

#[test]
fn picker_enter_in_interaction_mode_requires_selected_recipient() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    // Interaction entry without a target auto-opens the picker on the session
    // column; with no recipients there is nothing to select.
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_target")
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn picker_enter_in_interaction_mode_attempts_look_for_selected_target() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    // The picker auto-opens on the session column with the lone recipient
    // selected, so Enter attempts a look against it.
    assert!(state.picker_open());
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "relay_unavailable")
        }
        Err(RuntimeError::Io { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied)
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn picker_typing_filters_focused_session_column() {
    let mut state = make_state();
    state.set_recipients(&["master", "mcp", "relay"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
    // Printable keys accumulate into the column-scoped filter and narrow the
    // session list to the first match.
    state
        .dispatch_event(key_event(KeyCode::Char('m'), KeyModifiers::NONE))
        .expect("filter char should be handled");
    state
        .dispatch_event(key_event(KeyCode::Char('c'), KeyModifiers::NONE))
        .expect("filter char should be handled");
    assert_eq!(state.picker_filter(), "mc");
    assert!(state.picker_open());
    // "mc" matches only "mcp"; Enter inserts it.
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should insert the filtered selection");
    assert_eq!(state.to_field(), "mcp");
    assert!(!state.picker_open());
}

#[test]
fn picker_tab_switches_focus_and_clears_filter() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker on the session column");
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
    state
        .dispatch_event(key_event(KeyCode::Char('z'), KeyModifiers::NONE))
        .expect("filter char should be handled");
    assert_eq!(state.picker_filter(), "z");
    state
        .dispatch_event(key_event(KeyCode::Tab, KeyModifiers::NONE))
        .expect("tab should switch picker focus");
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Bundles);
    assert_eq!(state.picker_filter(), "");
}

#[test]
fn picker_remembers_last_selected_across_close_and_reopen() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "bravo", "charlie"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves picker selection");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves picker selection");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter commits selection in communication mode");
    assert_eq!(state.last_selected_recipient(), Some("charlie"));
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should reopen picker");
    assert_eq!(state.picker_selected_index(), Some(2));
}

#[test]
fn picker_restores_last_selected_after_relay_refresh_reorders_recipients() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "bravo", "charlie"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves picker selection");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter commits selection in communication mode");
    assert_eq!(state.last_selected_recipient(), Some("bravo"));
    state.set_recipients(&["charlie", "bravo", "alpha", "delta"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should reopen picker");
    assert_eq!(state.picker_selected_index(), Some(1));
}

#[test]
fn picker_falls_back_to_first_when_last_selected_is_absent_after_refresh() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "bravo", "charlie"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves picker selection");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves picker selection");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter commits selection in communication mode");
    assert_eq!(state.last_selected_recipient(), Some("charlie"));
    state.set_recipients(&["alpha", "bravo", "delta"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should reopen picker");
    assert_eq!(state.picker_selected_index(), Some(0));
}

#[test]
fn picker_has_no_selection_when_recipient_list_is_empty() {
    let mut state = make_state();
    state.set_recipients(&["alpha"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter commits selection in communication mode");
    state.set_recipients(&[]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should reopen picker");
    assert_eq!(state.picker_selected_index(), None);
}
