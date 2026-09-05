//! The picker overlay's bundle column: the F5 toggle, which bundle is
//! highlighted on open, and what a bundle switch resets.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::{
    runtime::error::RuntimeError,
    tui::{
        TuiLaunchOptions,
        workbench::{Workbench, WorkbenchPickerColumn},
    },
};

use super::{key_event, make_state};

#[test]
fn f5_toggles_unified_picker_on_bundle_column() {
    let mut state = make_state();
    assert!(!state.picker_open());
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should open the unified picker");
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Bundles);
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should close the unified picker");
    assert!(!state.picker_open());
}

#[test]
fn bundle_picker_highlights_active_bundle_on_open() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should open bundle picker");
    let active_index = state
        .available_bundles()
        .iter()
        .position(|name| *name == state.namespace())
        .expect("active bundle should appear in available_bundles");
    assert_eq!(state.bundle_picker_selected_index(), Some(active_index));
}

#[test]
fn bundle_picker_enter_on_active_bundle_hands_focus_to_sessions() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "bravo"]);
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should open bundle picker");
    let original_bundle = state.namespace().to_string();
    let original_recipients = state.recipients();
    let original_recipients_owned = original_recipients
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter on the active bundle should be a no-op switch");
    // Selecting the active bundle does not switch context; it keeps the picker
    // open and hands focus to the session column in the same window.
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
    assert_eq!(state.namespace(), original_bundle.as_str());
    assert_eq!(
        state
            .recipients()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        original_recipients_owned
    );
}

#[test]
fn bundle_picker_enter_on_different_bundle_switches_and_resets_bundle_scoped_state() {
    let mut state = make_state();
    state.set_recipients(&["alpha", "bravo"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open recipient picker");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter in communication mode should insert recipient");
    assert_eq!(state.last_selected_recipient(), Some("alpha"));
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should open bundle picker");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down moves bundle picker selection");
    let target_index = state
        .bundle_picker_selected_index()
        .expect("bundle picker should have a selection");
    let target_bundle = state.available_bundles()[target_index].to_string();
    assert_ne!(target_bundle, state.namespace());
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(state.namespace(), target_bundle.as_str());
    assert!(state.recipients().is_empty());
    assert_eq!(state.last_selected_recipient(), None);
    // The switch keeps the picker open and hands focus to the (re-enumerated)
    // session column so a session can be picked in the same window.
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Sessions);
    match result {
        Err(RuntimeError::Validation { code, .. }) => assert_eq!(code, "relay_unavailable"),
        Err(RuntimeError::Io { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied)
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn bundle_picker_enter_with_no_available_bundles_returns_validation_error() {
    let mut state = Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
        look_lines: None,
        available_bundles: Vec::new(),
        bindings: None,
    });
    state
        .dispatch_event(key_event(KeyCode::F(5), KeyModifiers::NONE))
        .expect("f5 should open bundle picker");
    assert!(state.picker_open());
    assert_eq!(state.picker_column(), WorkbenchPickerColumn::Bundles);
    assert_eq!(state.bundle_picker_selected_index(), None);
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_target")
        }
        other => panic!("unexpected result: {other:?}"),
    }
}
