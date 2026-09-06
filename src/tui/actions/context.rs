//! Resolution of the binding contexts a chord is looked up against.

use super::super::state::{AppState, FocusField, PickerColumn, ScreenMode};

/// Counts the identifiers handed to it, so a generated list can keep its
/// fixed-size array type rather than decaying to a slice.
macro_rules! count_identifiers {
    () => (0usize);
    ($head:ident $(, $tail:ident)*) => (1usize + count_identifiers!($($tail),*));
}

/// Declares the binding contexts once, so the enum, the list of every context,
/// and their operator-facing names cannot disagree.
///
/// Completeness is the reason this is a macro. An exhaustive `match` forces a
/// new context to be *considered*, but nothing forces it into a separate list,
/// and a context missing from that list would carry a name
/// [`BindingContext::from_configuration_name`] could never find while every
/// test that walks the list still passed. Generating all three from one place
/// removes the seam: a context exists only by appearing here.
///
/// [`BindingContext::position`] stays hand-written and exhaustive rather than
/// being derived from `ALL`. Deriving it would make the test that asserts the
/// two agree tautological, and that test is what catches a context declared in
/// one order and positioned in another.
macro_rules! declare_binding_contexts {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => $name:expr,
        )+
    ) => {
        /// A key under which binding rows are declared.
        ///
        /// All but [`BindingContext::Global`] name a surface the operator can
        /// be on. Overlay surfaces outrank screen-mode surfaces, and within a
        /// screen mode the focused field selects the surface. Holding that as a
        /// value rather than as an ordering of handler early-returns is what
        /// makes the precedence assertable without simulating dispatch.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum BindingContext {
            $(
                $(#[$meta])*
                $variant,
            )+
        }

        impl BindingContext {
            /// Every context, so a caller can ask what the defaults say across
            /// the whole surface rather than context by context. Complete by
            /// construction: this and the enum come from one declaration.
            pub const ALL: [BindingContext; count_identifiers!($($variant),+)] = [
                $( BindingContext::$variant ),+
            ];

            /// This context's name in an operator's binding configuration.
            ///
            /// Kebab-case, and deliberately not the variant identifier: a
            /// configuration is written by someone who has not read this
            /// source, so the spelling is part of the operator-facing surface
            /// rather than an incidental echo of the internal name.
            #[must_use]
            pub const fn configuration_name(self) -> &'static str {
                match self {
                    $( Self::$variant => $name, )+
                }
            }

        }
    };
}

declare_binding_contexts! {
    /// Rows that hold whichever surface is active, because the behavior they
    /// reach is not the surface's to own: quitting, and the help overlay the
    /// operator must be able to summon from anywhere. Resolved ahead of the
    /// contextual rows, so an open surface cannot shadow them.
    Global => "global",
    PickerBundles => "picker-bundles",
    PickerSessions => "picker-sessions",
    EventsOverlay => "events-overlay",
    HelpOverlay => "help-overlay",
    ComposeTo => "compose-to",
    ComposeMessage => "compose-message",
    InteractionChoice => "interaction-choice",
    InteractionWrite => "interaction-write",
}

impl BindingContext {
    /// The context a configuration name denotes, if any does.
    ///
    /// Derived by searching [`BindingContext::ALL`] rather than by a second
    /// match, so the forward and reverse spellings cannot drift apart.
    #[must_use]
    pub fn from_configuration_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|context| context.configuration_name() == name)
    }

    /// This context's index in [`BindingContext::ALL`].
    ///
    /// Hand-written rather than derived by searching `ALL`, so that the test
    /// asserting the two agree has something to catch: a position derived from
    /// the list it is checked against could never disagree with it.
    #[must_use]
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
/// Resolution stops at the first context binding the chord to an action, which
/// is narrower than the first that declares it: a context binding the chord to
/// no action empties it there and lets the next be consulted, so an explicit
/// unbinding uncovers the surface row a global row was shadowing rather than
/// silencing the key everywhere.
///
/// Dispatch reads this rather than testing chords ahead of the table, so a
/// globally bound chord keeps its action with any surface open and stays
/// declared in exactly one place.
pub(crate) fn binding_lookup_order(state: &AppState) -> [BindingContext; 2] {
    lookup_order(binding_context(state))
}

/// The same order, for a caller that has a context rather than a state.
///
/// Dispatch resolves the surface from state; asking what a configuration leaves
/// reachable means asking about every surface, with no state to resolve one
/// from. Both read this, because a caller that restated the order and asked one
/// context alone would count a chord a global row has taken as still reaching
/// the surface's row.
///
/// [`BindingContext::Global`] passed here yields itself twice, which answers
/// exactly as consulting it once does. Left to fall out rather than special
/// cased: the global rows are consulted first whatever the surface, and that is
/// as true when they are the surface.
pub(crate) const fn lookup_order(context: BindingContext) -> [BindingContext; 2] {
    [BindingContext::Global, context]
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
