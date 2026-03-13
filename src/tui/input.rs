use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::runtime::error::RuntimeError;

use super::state::{AppState, FocusField};

pub(crate) fn handle_event(state: &mut AppState, event: Event) -> Result<(), RuntimeError> {
    match event {
        Event::Key(key) => handle_key(state, key),
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

    if state.picker_open {
        return handle_picker_key(state, key);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('q') => {
                state.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('s') => return state.send_message(),
            KeyCode::Char('l') => return state.look_target(),
            KeyCode::Char('r') => return state.refresh_recipients(),
            KeyCode::Char('d') => {
                state.toggle_delivery_mode();
                return Ok(());
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => {
            state.should_quit = true;
        }
        KeyCode::F(2) => {
            state.open_picker();
        }
        KeyCode::BackTab => state.cycle_focus(),
        KeyCode::Tab => {
            if matches!(state.focus, FocusField::To | FocusField::Cc) {
                state.autocomplete_active_recipient_field();
            }
        }
        KeyCode::Enter => state.insert_newline_if_message(),
        KeyCode::Backspace => state.backspace(),
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
        KeyCode::Down => state.move_picker_selection(1),
        KeyCode::Up => state.move_picker_selection(-1),
        KeyCode::Enter => state.insert_picker_selection(),
        _ => {}
    }
    Ok(())
}
