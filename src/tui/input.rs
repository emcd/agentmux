//! Terminal event handling.
//!
//! This module owns only what is event-shaped: which events carry a binding at
//! all, and how a paste or a scroll reaches state. Which chord means what is the
//! binding table's, and every key that acts does so by resolving against it.
//! No chord is named here, so none can acquire behavior or reach outside the
//! table.

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEventKind};

use crate::runtime::error::RuntimeError;

use super::actions::{Action, binding_lookup_order};
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
///
/// Resolution goes through the effective table, so what the operator configured
/// answers ahead of what ships. Precedence between contexts is unchanged and
/// stays here: the table answers for one context at a time, and a global row
/// outranks a contextual one because this asks the global context first.
///
/// An explicit unbinding empties the chord in the context that named it, and
/// the next context is then consulted as it would be for a chord that context
/// never bound. That is the same scoping every other row has -- a row belongs
/// to one context -- so unbinding a global chord uncovers a surface row that
/// the global row was shadowing, rather than silencing the key everywhere.
fn resolve_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    binding_lookup_order(state)
        .into_iter()
        .find_map(|context| state.bindings.action_for(context, key.code, key.modifiers))
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
