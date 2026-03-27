use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use agentmux::{
    runtime::error::RuntimeError,
    tui::{
        TuiLaunchOptions,
        workbench::{Workbench, WorkbenchField},
    },
};

fn make_state() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        bundle_name: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
        look_lines: None,
    })
}

fn key_event(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

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
fn esc_in_message_snaps_history_to_latest() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.inject_outgoing_history_entry("oldest");
    state.inject_outgoing_history_entry("newest");
    state.set_chat_history_viewport_height(1);
    state.scroll_chat_history_page_up();
    assert_eq!(state.visible_chat_history_bodies(), vec!["oldest"]);
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc should be handled");
    assert_eq!(state.visible_chat_history_bodies(), vec!["newest"]);
}

#[test]
fn mouse_wheel_scrolls_history() {
    let mut state = make_state();
    state.inject_outgoing_history_entry("oldest");
    state.inject_outgoing_history_entry("newest");
    state.set_chat_history_viewport_height(1);

    let scroll_up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    state.dispatch_event(scroll_up).expect("scroll up");
    assert_eq!(state.visible_chat_history_bodies(), vec!["oldest"]);

    let scroll_down = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    state.dispatch_event(scroll_down).expect("scroll down");
    assert_eq!(state.visible_chat_history_bodies(), vec!["newest"]);
}

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
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should be handled");
    assert_eq!(state.to_field(), "master");
    for _ in 0..5 {
        state
            .dispatch_event(key_event(KeyCode::Backspace, KeyModifiers::NONE))
            .expect("backspace should be handled");
    }
    assert_eq!(state.to_field(), "m");
    state
        .dispatch_event(key_event(KeyCode::Char(' '), KeyModifiers::CONTROL))
        .expect("ctrl+space should retrigger completion mode");
    state
        .dispatch_event(key_event(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should cycle in active completion mode");
    assert_eq!(state.to_field(), "mcp");
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
