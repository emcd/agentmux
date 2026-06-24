use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use agentmux::{
    runtime::error::RuntimeError,
    tui::{
        TuiLaunchOptions,
        workbench::{Workbench, WorkbenchField, WorkbenchMode, WorkbenchPickerColumn},
    },
};

fn make_state() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string(), "secondary".to_string()],
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
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");
    assert_eq!(state.mode(), WorkbenchMode::Interaction);
    assert!(!state.picker_open());
}

#[test]
fn interaction_region_swaps_between_raww_and_choice_pane() {
    let mut state = make_state();
    state.set_recipients(&["master"]);
    state.set_interaction_target("master");
    state
        .dispatch_event(key_event(KeyCode::F(4), KeyModifiers::NONE))
        .expect("f4 should switch to interaction");

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
fn same_session_pending_choices_order_fifo_by_enqueued_at() {
    let mut state = make_state();
    // Requests arrive for one session out of enqueued_at order (the relay may
    // replay or reorder on the wire); the pending list must present them FIFO by
    // enqueued_at regardless of arrival order.
    state.inject_choice_request("req-c", "acp@agentmux", Some("2026-06-24T00:00:03Z"));
    state.inject_choice_request("req-a", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-b", "acp@agentmux", Some("2026-06-24T00:00:02Z"));

    assert_eq!(
        state.pending_choice_request_ids(),
        vec!["req-a", "req-b", "req-c"],
        "pending choices must be ordered FIFO by enqueued_at"
    );
}

#[test]
fn pending_choices_tie_break_by_request_id_and_sink_missing_enqueued_at() {
    let mut state = make_state();
    // Two requests share an enqueued_at: ties break deterministically by
    // choice_request_id. A request with no enqueued_at sorts last.
    state.inject_choice_request("req-z", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-a", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-m", "acp@agentmux", None);

    assert_eq!(
        state.pending_choice_request_ids(),
        vec!["req-a", "req-z", "req-m"],
        "equal enqueued_at ties break by choice_request_id; missing enqueued_at sorts last"
    );
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
