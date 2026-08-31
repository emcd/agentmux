//! Resolution of the binding contexts a chord is looked up against.

use super::super::state::{AppState, FocusField, PickerColumn, ScreenMode};

/// A key under which binding rows are declared.
///
/// All but [`BindingContext::Global`] name a surface the operator can be on.
/// Overlay surfaces outrank screen-mode surfaces, and within a screen mode the
/// focused field selects the surface. Holding that as a value rather than as an
/// ordering of handler early-returns is what makes the precedence assertable
/// without simulating dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum BindingContext {
    /// Rows that hold whichever surface is active, because the behavior they
    /// reach is not the surface's to own: quitting, and the help overlay the
    /// operator must be able to summon from anywhere. Resolved ahead of the
    /// contextual rows, so an open surface cannot shadow them.
    Global,
    PickerBundles,
    PickerSessions,
    EventsOverlay,
    HelpOverlay,
    ComposeTo,
    ComposeMessage,
    InteractionChoice,
    InteractionWrite,
}

/// The contexts a chord is resolved against, in precedence order: the global
/// rows first, then the one contextual owner.
///
/// Resolution stops at the first context declaring the chord. Dispatch reads
/// this rather than testing chords ahead of the table, so a globally bound
/// chord keeps its action with any surface open and stays declared in exactly
/// one place.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn binding_lookup_order(state: &AppState) -> [BindingContext; 2] {
    [BindingContext::Global, binding_context(state)]
}

/// Resolves the contextual owner — the surface whose rows are consulted after
/// the global ones. Never returns [`BindingContext::Global`], which is not a
/// surface the operator can be on.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn binding_context(state: &AppState) -> BindingContext {
    if state.picker_open {
        return match state.picker_focus {
            PickerColumn::Bundles => BindingContext::PickerBundles,
            PickerColumn::Sessions => BindingContext::PickerSessions,
        };
    }
    if state.events_overlay_open {
        return BindingContext::EventsOverlay;
    }
    if state.help_overlay_open {
        return BindingContext::HelpOverlay;
    }
    match state.mode {
        ScreenMode::Communication => match state.focus {
            FocusField::To => BindingContext::ComposeTo,
            FocusField::Message => BindingContext::ComposeMessage,
        },
        ScreenMode::Interaction => {
            if state.interaction_choice_active() {
                BindingContext::InteractionChoice
            } else {
                BindingContext::InteractionWrite
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::super::state::{PendingChoiceEntry, TuiLaunchOptions};
    use super::{
        AppState, BindingContext, FocusField, PickerColumn, ScreenMode, binding_context,
        binding_lookup_order,
    };

    fn workbench_state() -> AppState {
        AppState::new(TuiLaunchOptions {
            namespace: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: PathBuf::from("/tmp/agentmux-binding-context-test.sock"),
            look_lines: None,
            available_bundles: Vec::new(),
        })
    }

    fn pending_choice(target: &str) -> PendingChoiceEntry {
        PendingChoiceEntry {
            choice_request_id: format!("choice-{target}"),
            message_id: None,
            target_session: Some(target.to_string()),
            requested_kind: None,
            requested_details: None,
            enqueued_at: None,
            options: Vec::new(),
        }
    }

    #[test]
    fn resolves_globals_first_then_overlay_precedence_then_the_focused_field() {
        for mode in [ScreenMode::Communication, ScreenMode::Interaction] {
            let mut state = workbench_state();
            state.mode = mode;
            state.picker_open = true;
            state.events_overlay_open = true;
            state.help_overlay_open = true;
            state.picker_focus = PickerColumn::Bundles;
            assert_eq!(binding_context(&state), BindingContext::PickerBundles);
            state.picker_focus = PickerColumn::Sessions;
            assert_eq!(binding_context(&state), BindingContext::PickerSessions);
            state.picker_open = false;
            assert_eq!(binding_context(&state), BindingContext::EventsOverlay);
            state.events_overlay_open = false;
            assert_eq!(binding_context(&state), BindingContext::HelpOverlay);
        }
        // A globally bound chord must not be shadowed by whatever surface is
        // open, so the global rows lead the lookup in every state -- including
        // each surface that would otherwise claim the chord for itself.
        for surface in [
            BindingContext::PickerBundles,
            BindingContext::PickerSessions,
            BindingContext::EventsOverlay,
            BindingContext::HelpOverlay,
            BindingContext::ComposeTo,
        ] {
            let mut state = workbench_state();
            match surface {
                BindingContext::PickerBundles => {
                    state.picker_open = true;
                    state.picker_focus = PickerColumn::Bundles;
                }
                BindingContext::PickerSessions => {
                    state.picker_open = true;
                    state.picker_focus = PickerColumn::Sessions;
                }
                BindingContext::EventsOverlay => state.events_overlay_open = true,
                BindingContext::HelpOverlay => state.help_overlay_open = true,
                _ => state.focus = FocusField::To,
            }
            assert_eq!(
                binding_lookup_order(&state),
                [BindingContext::Global, surface],
                "global rows must lead the lookup with {surface:?} active"
            );
            assert_ne!(
                binding_context(&state),
                BindingContext::Global,
                "the contextual owner is a surface, never the global rows"
            );
        }
        let mut state = workbench_state();
        state.focus = FocusField::To;
        assert_eq!(binding_context(&state), BindingContext::ComposeTo);
        state.focus = FocusField::Message;
        assert_eq!(binding_context(&state), BindingContext::ComposeMessage);
        state.mode = ScreenMode::Interaction;
        assert_eq!(binding_context(&state), BindingContext::InteractionWrite);
        state.look_target = Some("alpha".to_string());
        state.pending_choices.push(pending_choice("alpha"));
        assert_eq!(binding_context(&state), BindingContext::InteractionChoice);
        state.raww_draft.push('x');
        assert_eq!(binding_context(&state), BindingContext::InteractionWrite);
    }
}
