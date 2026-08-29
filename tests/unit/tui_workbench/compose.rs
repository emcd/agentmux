//! Compose-surface editing: focus movement between the To and Message
//! fields, and cursor/edit behaviour within each.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::{runtime::error::RuntimeError, tui::workbench::WorkbenchField};

use super::{key_event, make_state};

#[test]
fn enter_in_message_triggers_send_path() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("hello");
    let result = state.dispatch_event(key_event(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_empty_targets")
        }
        other => panic!("unexpected result: {other:?}"),
    }
    assert_eq!(state.message_field(), "hello");
}

#[test]
fn tab_moves_focus_without_to_completion() {
    let mut state = make_state();
    state.insert_text("ag");
    state
        .dispatch_event(key_event(KeyCode::Tab, KeyModifiers::NONE))
        .expect("tab should be handled");
    assert_eq!(state.focus(), WorkbenchField::Message);
    assert_eq!(state.to_field(), "ag");
}

#[test]
fn ctrl_j_inserts_newline_in_message_field() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("hello");
    state
        .dispatch_event(key_event(KeyCode::Char('j'), KeyModifiers::CONTROL))
        .expect("ctrl+j should be handled");
    assert_eq!(state.message_field(), "hello\n");
}

#[test]
fn shift_enter_does_not_send_or_insert_newline() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("hello");
    state
        .dispatch_event(key_event(KeyCode::Enter, KeyModifiers::SHIFT))
        .expect("shift+enter should be handled");
    assert_eq!(state.message_field(), "hello");
}

#[test]
fn to_field_left_arrow_inserts_at_cursor() {
    let mut state = make_state();
    state.insert_text("abc");
    assert_eq!(state.to_cursor_column(), 3);
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left should be handled");
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left should be handled");
    assert_eq!(state.to_cursor_column(), 1);
    state.insert_text("X");
    assert_eq!(state.to_field(), "aXbc");
    assert_eq!(state.to_cursor_column(), 2);
}

#[test]
fn to_field_right_arrow_advances_cursor() {
    let mut state = make_state();
    state.insert_text("abc");
    state
        .dispatch_event(key_event(KeyCode::Home, KeyModifiers::NONE))
        .expect("home should be handled");
    assert_eq!(state.to_cursor_column(), 0);
    state
        .dispatch_event(key_event(KeyCode::Right, KeyModifiers::NONE))
        .expect("right should be handled");
    assert_eq!(state.to_cursor_column(), 1);
}

#[test]
fn to_field_ctrl_a_and_ctrl_e_jump_to_bounds() {
    let mut state = make_state();
    state.insert_text("abc");
    state
        .dispatch_event(key_event(KeyCode::Char('a'), KeyModifiers::CONTROL))
        .expect("ctrl+a should be handled");
    assert_eq!(state.to_cursor_column(), 0);
    state
        .dispatch_event(key_event(KeyCode::Char('e'), KeyModifiers::CONTROL))
        .expect("ctrl+e should be handled");
    assert_eq!(state.to_cursor_column(), 3);
}

#[test]
fn to_field_ctrl_u_clears_the_line() {
    let mut state = make_state();
    state.insert_text("master, mcp");
    state
        .dispatch_event(key_event(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .expect("ctrl+u should be handled");
    assert_eq!(state.to_field(), "");
    assert_eq!(state.to_cursor_column(), 0);
}

#[test]
fn to_field_backspace_deletes_before_cursor() {
    let mut state = make_state();
    state.insert_text("abc");
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left should be handled");
    state
        .dispatch_event(key_event(KeyCode::Backspace, KeyModifiers::NONE))
        .expect("backspace should be handled");
    assert_eq!(state.to_field(), "ac");
    assert_eq!(state.to_cursor_column(), 1);
}

#[test]
fn message_cursor_moves_vertically_without_history_recall() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("abc\nde\nfghi");
    assert_eq!(state.message_cursor_line_and_column(), (2, 4));
    state
        .dispatch_event(key_event(KeyCode::Up, KeyModifiers::NONE))
        .expect("up should be handled");
    assert_eq!(state.message_cursor_line_and_column(), (1, 2));
    state
        .dispatch_event(key_event(KeyCode::Up, KeyModifiers::NONE))
        .expect("up should be handled");
    assert_eq!(state.message_cursor_line_and_column(), (0, 3));
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should be handled");
    assert_eq!(state.message_cursor_line_and_column(), (1, 2));
}

#[test]
fn message_cursor_supports_horizontal_arrow_and_home_end_navigation() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("abcd");

    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left should move cursor");
    state
        .dispatch_event(key_event(KeyCode::Left, KeyModifiers::NONE))
        .expect("left should move cursor");
    state
        .dispatch_event(key_event(KeyCode::Char('X'), KeyModifiers::NONE))
        .expect("insert should honor moved cursor");
    assert_eq!(state.message_field(), "abXcd");

    state
        .dispatch_event(key_event(KeyCode::Home, KeyModifiers::NONE))
        .expect("home should move to line start");
    state
        .dispatch_event(key_event(KeyCode::Char('^'), KeyModifiers::NONE))
        .expect("insert at start should work");
    assert_eq!(state.message_field(), "^abXcd");

    state
        .dispatch_event(key_event(KeyCode::End, KeyModifiers::NONE))
        .expect("end should move to line end");
    state
        .dispatch_event(key_event(KeyCode::Char('$'), KeyModifiers::NONE))
        .expect("insert at end should work");
    assert_eq!(state.message_field(), "^abXcd$");
}

#[test]
fn message_cursor_supports_readline_ctrl_a_ctrl_e_navigation() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.insert_text("hello");

    state
        .dispatch_event(key_event(KeyCode::Char('a'), KeyModifiers::CONTROL))
        .expect("ctrl+a should move to line start");
    state
        .dispatch_event(key_event(KeyCode::Char('>'), KeyModifiers::NONE))
        .expect("insert at start should work");
    assert_eq!(state.message_field(), ">hello");

    state
        .dispatch_event(key_event(KeyCode::Char('e'), KeyModifiers::CONTROL))
        .expect("ctrl+e should move to line end");
    state
        .dispatch_event(key_event(KeyCode::Char('<'), KeyModifiers::NONE))
        .expect("insert at end should work");
    assert_eq!(state.message_field(), ">hello<");
}
