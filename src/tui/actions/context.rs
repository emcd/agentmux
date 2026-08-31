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
pub enum BindingContext {
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

impl BindingContext {
    /// Every context, so a caller can ask what the defaults say across the
    /// whole surface rather than context by context. Exhaustive by
    /// construction: [`BindingContext::position`] matches on every variant, so
    /// a new one cannot be added without being placed here too.
    pub const ALL: [BindingContext; 9] = [
        BindingContext::Global,
        BindingContext::PickerBundles,
        BindingContext::PickerSessions,
        BindingContext::EventsOverlay,
        BindingContext::HelpOverlay,
        BindingContext::ComposeTo,
        BindingContext::ComposeMessage,
        BindingContext::InteractionChoice,
        BindingContext::InteractionWrite,
    ];

    /// This context's index in [`BindingContext::ALL`].
    pub const fn position(self) -> usize {
        match self {
            Self::Global => 0,
            Self::PickerBundles => 1,
            Self::PickerSessions => 2,
            Self::EventsOverlay => 3,
            Self::HelpOverlay => 4,
            Self::ComposeTo => 5,
            Self::ComposeMessage => 6,
            Self::InteractionChoice => 7,
            Self::InteractionWrite => 8,
        }
    }
}

/// The contexts a chord is resolved against, in precedence order: the global
/// rows first, then the one contextual owner.
///
/// Resolution stops at the first context declaring the chord. Dispatch reads
/// this rather than testing chords ahead of the table, so a globally bound
/// chord keeps its action with any surface open and stays declared in exactly
/// one place.
pub(crate) fn binding_lookup_order(state: &AppState) -> [BindingContext; 2] {
    [BindingContext::Global, binding_context(state)]
}

/// Resolves the contextual owner — the surface whose rows are consulted after
/// the global ones. Never returns [`BindingContext::Global`], which is not a
/// surface the operator can be on.
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
