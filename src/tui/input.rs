use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::runtime::error::RuntimeError;

use super::state::{AppState, FocusField, ScreenMode};

pub(crate) fn handle_event(state: &mut AppState, event: Event) -> Result<(), RuntimeError> {
    match event {
        Event::Key(key) => handle_key(state, key),
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => state.scroll_chat_history_up(),
                MouseEventKind::ScrollDown => state.scroll_chat_history_down(),
                _ => {}
            }
            Ok(())
        }
        Event::Paste(text) => {
            insert_text_for_active_mode(state, text.as_str());
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    if key.kind != KeyEventKind::Press {
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        state.should_quit = true;
        return Ok(());
    }

    if key.code == KeyCode::F(1) {
        state.toggle_help_overlay();
        return Ok(());
    }

    if state.picker_open {
        return handle_picker_key(state, key);
    }
    if state.bundle_picker_open {
        return handle_bundle_picker_key(state, key);
    }
    if state.events_overlay_open {
        return handle_events_overlay_key(state, key);
    }
    if state.help_overlay_open {
        return handle_help_overlay_key(state, key);
    }

    if key.code == KeyCode::F(4) {
        state.toggle_mode();
        return Ok(());
    }

    if key.code == KeyCode::F(5) {
        state.open_bundle_picker();
        return Ok(());
    }

    match state.mode {
        ScreenMode::Communication => handle_communication_mode_key(state, key),
        ScreenMode::Interaction => handle_interaction_mode_key(state, key),
    }
}

fn handle_communication_mode_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('a') if state.focus == FocusField::Message => {
                state.move_message_cursor_home();
                return Ok(());
            }
            KeyCode::Char('e') if state.focus == FocusField::Message => {
                state.move_message_cursor_end();
                return Ok(());
            }
            KeyCode::Char('j') => {
                state.insert_newline_if_message();
                return Ok(());
            }
            KeyCode::Char('r') => return state.refresh_recipients(),
            KeyCode::Char(' ') => state.autocomplete_active_recipient_field(),
            _ => {}
        }
    }

    match key.code {
        KeyCode::F(2) => {
            if state.picker_open {
                state.close_picker();
            } else {
                state.open_picker();
            }
        }
        KeyCode::F(3) => state.toggle_events_overlay(),
        KeyCode::BackTab => state.cycle_focus_backward(),
        KeyCode::Tab => state.cycle_focus_forward(),
        KeyCode::Enter if key.modifiers.is_empty() => {
            if state.focus == FocusField::To {
                state.accept_active_to_completion();
            } else if state.focus == FocusField::Message {
                return state.send_message();
            }
        }
        KeyCode::Esc if state.focus == FocusField::Message => {
            state.snap_chat_history_to_latest();
        }
        KeyCode::Up => {
            if state.focus == FocusField::To {
                state.move_to_completion_selection(-1);
            } else if state.focus == FocusField::Message {
                state.move_message_cursor_up();
            }
        }
        KeyCode::Down => {
            if state.focus == FocusField::To {
                state.move_to_completion_selection(1);
            } else if state.focus == FocusField::Message {
                state.move_message_cursor_down();
            }
        }
        KeyCode::Left if state.focus == FocusField::Message => {
            state.move_message_cursor_left();
        }
        KeyCode::Right if state.focus == FocusField::Message => {
            state.move_message_cursor_right();
        }
        KeyCode::Home if state.focus == FocusField::Message => {
            state.move_message_cursor_home();
        }
        KeyCode::End if state.focus == FocusField::Message => {
            state.move_message_cursor_end();
        }
        KeyCode::Backspace => state.backspace(),
        KeyCode::PageUp => state.scroll_chat_history_page_up(),
        KeyCode::PageDown => state.scroll_chat_history_page_down(),
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            state.insert_character(character);
        }
        _ => {}
    }

    Ok(())
}

fn handle_interaction_mode_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('j') => {
                state.insert_newline_in_raww();
                return Ok(());
            }
            KeyCode::Char('r') => return state.refresh_recipients(),
            _ => {}
        }
    }

    match key.code {
        KeyCode::F(2) => state.open_picker(),
        KeyCode::F(3) => state.toggle_events_overlay(),
        KeyCode::PageUp => state.scroll_interaction_snapshot_page_up(),
        KeyCode::PageDown => state.scroll_interaction_snapshot_page_down(),
        KeyCode::Left => {
            if interaction_choice_active(state) {
                state.move_look_choice_request_selection(-1);
            } else {
                state.move_raww_cursor_left();
            }
        }
        KeyCode::Right => {
            if interaction_choice_active(state) {
                state.move_look_choice_request_selection(1);
            } else {
                state.move_raww_cursor_right();
            }
        }
        KeyCode::Up => {
            if interaction_choice_active(state) {
                state.move_look_choice_option_selection(-1);
            } else if !state.raww_draft.is_empty() {
                state.move_raww_cursor_up();
            } else {
                state.scroll_interaction_snapshot_up();
            }
        }
        KeyCode::Down => {
            if interaction_choice_active(state) {
                state.move_look_choice_option_selection(1);
            } else if !state.raww_draft.is_empty() {
                state.move_raww_cursor_down();
            } else {
                state.scroll_interaction_snapshot_down();
            }
        }
        KeyCode::Home if !interaction_choice_active(state) => {
            state.move_raww_cursor_home();
        }
        KeyCode::End if !interaction_choice_active(state) => {
            state.move_raww_cursor_end();
        }
        KeyCode::Enter => {
            if interaction_choice_active(state) {
                return state.resolve_selected_look_choice_selected();
            }
            return state.dispatch_raww_from_interaction();
        }
        KeyCode::Backspace => state.backspace_raww(),
        KeyCode::Char(character)
            if interaction_choice_active(state)
                && (character == 'c' || character == 'C')
                && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
        {
            return state.resolve_selected_look_choice_cancelled();
        }
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            state.insert_character_in_raww(character);
        }
        _ => {}
    }

    Ok(())
}

fn interaction_choice_active(state: &AppState) -> bool {
    state.raww_draft.is_empty() && !state.look_pending_choices().is_empty()
}

fn insert_text_for_active_mode(state: &mut AppState, text: &str) {
    match state.mode {
        ScreenMode::Communication => state.insert_text(text),
        ScreenMode::Interaction => {
            for character in text.chars() {
                state.insert_character_in_raww(character);
            }
        }
    }
}

fn handle_picker_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc | KeyCode::F(2) => state.close_picker(),
        KeyCode::F(3) => {
            state.close_picker();
            state.toggle_events_overlay();
        }
        KeyCode::F(4) => {
            state.close_picker();
            state.toggle_mode();
        }
        KeyCode::F(5) => {
            state.close_picker();
            state.open_bundle_picker();
        }
        KeyCode::Down => state.move_picker_selection(1),
        KeyCode::Up => state.move_picker_selection(-1),
        KeyCode::Enter => match state.mode {
            ScreenMode::Communication => state.insert_picker_selection(),
            ScreenMode::Interaction => return state.enter_interaction_from_picker(),
        },
        _ => {}
    }
    Ok(())
}

fn handle_bundle_picker_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc | KeyCode::F(5) => state.close_bundle_picker(),
        KeyCode::F(2) => {
            state.close_bundle_picker();
            state.open_picker();
        }
        KeyCode::F(3) => {
            state.close_bundle_picker();
            state.toggle_events_overlay();
        }
        KeyCode::F(4) => {
            state.close_bundle_picker();
            state.toggle_mode();
        }
        KeyCode::Down => state.move_bundle_picker_selection(1),
        KeyCode::Up => state.move_bundle_picker_selection(-1),
        KeyCode::Enter => return state.switch_to_selected_bundle(),
        _ => {}
    }
    Ok(())
}

fn handle_events_overlay_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc | KeyCode::F(3) => state.toggle_events_overlay(),
        KeyCode::F(2) => {
            state.toggle_events_overlay();
            state.open_picker();
        }
        KeyCode::F(4) => {
            state.toggle_events_overlay();
            state.toggle_mode();
        }
        KeyCode::F(5) => {
            state.toggle_events_overlay();
            state.open_bundle_picker();
        }
        _ => {}
    }
    Ok(())
}

fn handle_help_overlay_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc | KeyCode::F(1) => state.toggle_help_overlay(),
        KeyCode::F(2) => {
            state.toggle_help_overlay();
            state.open_picker();
        }
        KeyCode::F(3) => {
            state.toggle_help_overlay();
            state.toggle_events_overlay();
        }
        KeyCode::F(4) => {
            state.toggle_help_overlay();
            state.toggle_mode();
        }
        KeyCode::F(5) => {
            state.toggle_help_overlay();
            state.open_bundle_picker();
        }
        _ => {}
    }
    Ok(())
}
