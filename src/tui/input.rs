use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::runtime::error::RuntimeError;

use super::state::{AppState, FocusField};

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
            state.insert_text(text.as_str());
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
    if state.events_overlay_open {
        return handle_events_overlay_key(state, key);
    }
    if state.look_overlay_open {
        return handle_look_overlay_key(state, key);
    }
    if state.help_overlay_open {
        return handle_help_overlay_key(state, key);
    }

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

fn handle_picker_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc | KeyCode::F(2) => state.close_picker(),
        KeyCode::F(3) => {
            state.close_picker();
            state.toggle_events_overlay();
        }
        KeyCode::Down => state.move_picker_selection(1),
        KeyCode::Up => state.move_picker_selection(-1),
        KeyCode::Enter => state.insert_picker_selection(),
        KeyCode::Char(character)
            if (character == 'l' || character == 'L')
                && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
        {
            return state.look_picker_target();
        }
        KeyCode::Char(character)
            if (character == 'w' || character == 'W')
                && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
        {
            return state.raww_picker_target();
        }
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
        _ => {}
    }
    Ok(())
}

fn handle_look_overlay_key(state: &mut AppState, key: KeyEvent) -> Result<(), RuntimeError> {
    match key.code {
        KeyCode::Esc => state.close_look_overlay(),
        KeyCode::Up => state.scroll_look_overlay_up(),
        KeyCode::Down => state.scroll_look_overlay_down(),
        KeyCode::PageUp => state.scroll_look_overlay_page_up(),
        KeyCode::PageDown => state.scroll_look_overlay_page_down(),
        KeyCode::F(2) => {
            state.close_look_overlay();
            state.open_picker();
        }
        KeyCode::F(3) => {
            state.close_look_overlay();
            state.toggle_events_overlay();
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
        _ => {}
    }
    Ok(())
}
