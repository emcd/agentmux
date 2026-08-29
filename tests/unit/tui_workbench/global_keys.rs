//! Keys the workbench handles above its overlays: the global quit, and the
//! action keys an open overlay must swallow rather than act on.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{key_event, make_state};

#[test]
fn ctrl_c_quits_even_when_picker_overlay_is_open() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    state
        .dispatch_event(key_event(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .expect("ctrl+c should be handled globally");
    assert!(state.should_quit());
}

#[test]
fn events_overlay_choice_action_keys_are_ignored() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(3), KeyModifiers::NONE))
        .expect("f3 should open events overlay");
    state
        .dispatch_event(key_event(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("a should be ignored in events overlay");
    state
        .dispatch_event(key_event(KeyCode::Char('d'), KeyModifiers::NONE))
        .expect("d should be ignored in events overlay");
}
