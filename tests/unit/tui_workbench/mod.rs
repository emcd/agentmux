//! Feature-focused tests for the TUI workbench event surface:
//! - [`compose`]: To/Message field editing, focus, and cursor movement.
//! - [`completion`]: To-field recipient completion, including the
//!   cross-bundle candidate pool.
//! - [`chat_history`]: chat-history scrolling and viewport paging.
//! - [`modes`]: F4 mode switching, per-mode drafts, and the Interaction
//!   region's raww/choice-pane swap.
//! - [`session_picker`]: the session column of the picker overlay
//!   (selection, filtering, recipient insertion, selection memory).
//! - [`bundle_picker`]: the bundle column (F5, bundle switching, and the
//!   bundle-scoped state it resets).
//! - [`global_keys`]: keys handled above the overlays.
//! - [`help_overlay`]: the help overlay's viewport offset, its bounds, and the
//!   dismissal chords a scrolled overlay must not shadow.
//! - [`delivery`]: incoming-message dedupe and pending-delivery
//!   reconciliation against `delivery_outcome` events.
//! - [`choices`]: pending choice-request ordering, hydration, resolution,
//!   and the active-target filter.
//!
//! Helpers every module needs (the `Workbench` constructor, the key-event
//! builder, the relay stream-event builder and the UI address it targets)
//! live in this hub. Helpers only one module uses live with that module.

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use agentmux::{
    relay::RelayStreamEvent,
    tui::{TuiLaunchOptions, workbench::Workbench},
};

mod bundle_picker;
mod chat_history;
mod choices;
mod completion;
mod compose;
mod delivery;
mod global_keys;
mod help_overlay;
mod modes;
mod session_picker;

fn make_state() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string(), "secondary".to_string()],
        bindings: None,
    })
}

fn key_event(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

// The receiving UI session address the relay stamps on the envelope
// `target_session` of `incoming_message` and `choices.*` events pushed to this
// session (matches `make_state`). `delivery_outcome` envelopes instead name the
// delivery target, so those tests build the target address directly.
const UI_ADDRESS: &str = "tui@agentmux";

/// Builds a relay stream event with the given envelope target and payload,
/// mirroring the wire shape the relay emits (see `RelayStreamEvent`).
fn stream_event(
    event_type: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        target_session: target_session.to_string(),
        created_at: String::new(),
        payload,
    }
}
