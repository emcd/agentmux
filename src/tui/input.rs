//! Terminal event handling.
//!
//! This module owns only what is event-shaped: which events carry a binding at
//! all, and how a paste or a scroll reaches state. Which chord means what is the
//! binding table's, and every key that acts does so by resolving against it.
//! No chord is named here, so none can acquire behavior or reach outside the
//! table.

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEventKind};

use crate::runtime::error::RuntimeError;

use super::actions::{Action, binding_lookup_order, default_binding};
use super::state::{AppState, ScreenMode};

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
    let Some(action) = resolve_key(state, key) else {
        return Ok(());
    };
    action.apply(state)
}

/// Resolves a key against the contexts that own it right now, taking the first
/// that binds it. The lookup order puts the global rows ahead of the active
/// surface's, which is the whole of how a chord reaches across surfaces.
fn resolve_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    binding_lookup_order(state)
        .into_iter()
        .find_map(|context| default_binding(context, key.code, key.modifiers))
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
