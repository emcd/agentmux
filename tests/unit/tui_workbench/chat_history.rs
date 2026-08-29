//! Chat-history scrolling: mouse wheel, viewport-unit paging, and the
//! snap-to-latest escape.

use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};

use agentmux::tui::workbench::WorkbenchField;

use super::{key_event, make_state};

#[test]
fn esc_in_message_snaps_history_to_latest() {
    let mut state = make_state();
    state.set_focus(WorkbenchField::Message);
    state.inject_outgoing_history_entry("oldest");
    state.inject_outgoing_history_entry("newest");
    state.set_chat_history_viewport_height(2);
    state.set_chat_history_total_lines(8);
    state.scroll_chat_history_page_up();
    assert!(state.chat_history_scroll() > 0);
    state
        .dispatch_event(key_event(KeyCode::Esc, KeyModifiers::NONE))
        .expect("esc should be handled");
    assert_eq!(state.chat_history_scroll(), 0);
}

#[test]
fn mouse_wheel_scrolls_history() {
    let mut state = make_state();
    state.inject_outgoing_history_entry("oldest");
    state.inject_outgoing_history_entry("newest");
    state.set_chat_history_viewport_height(2);
    state.set_chat_history_total_lines(8);

    let scroll_up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    state.dispatch_event(scroll_up).expect("scroll up");
    assert_eq!(state.chat_history_scroll(), 1);

    let scroll_down = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    state.dispatch_event(scroll_down).expect("scroll down");
    assert_eq!(state.chat_history_scroll(), 0);
}

#[test]
fn chat_history_paging_moves_in_viewport_line_units() {
    let mut state = make_state();
    state.set_chat_history_viewport_height(4);
    state.set_chat_history_total_lines(10);

    state.scroll_chat_history_page_up();
    assert_eq!(state.chat_history_scroll(), 4);

    state.scroll_chat_history_page_up();
    assert_eq!(state.chat_history_scroll(), 6);

    state.scroll_chat_history_page_down();
    assert_eq!(state.chat_history_scroll(), 2);

    state.snap_chat_history_to_latest();
    assert_eq!(state.chat_history_scroll(), 0);
}
