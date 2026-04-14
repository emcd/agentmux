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

#[test]
fn f4_is_not_bound_on_main_workbench_surface() {
    let mut state = make_state();
    state.insert_text("master");
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should be ignored on main surface");
    assert_eq!(state.to_field(), "master");
}

#[test]
fn picker_look_requires_selected_recipient() {
    let mut state = make_state();
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    let result = state.dispatch_event(key_event(KeyCode::Char('l'), KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_target")
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn picker_look_uses_selected_recipient_target() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state
        .dispatch_event(key_event(KeyCode::F(2), KeyModifiers::NONE))
        .expect("f2 should open picker");
    let result = state.dispatch_event(key_event(KeyCode::Char('l'), KeyModifiers::NONE));
    match result {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "relay_unavailable")
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

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
