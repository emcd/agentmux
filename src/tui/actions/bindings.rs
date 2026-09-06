//! Default chord-to-action bindings, declared per binding context.
//!
//! The table this module owns is the single place a chord's action is
//! associated with the context that owns it, and the source dispatch, the help
//! overlay, the pane hint strips, and the operator usage guide all read.
//!
//! Rows are grouped under their context rather than repeating it: the context
//! is the key a group is filed under, and within a group declaration order is
//! the tiebreak between rows that could both match. The global group holds the
//! rows that survive any open surface, and dispatch consults it first.
//!
//! Rows carry no capability field. Nothing varies by keyboard-enhancement probe
//! outcome: a modified `Enter` invokes whatever its context binds to bare
//! `Enter`, and invokes nothing in the two overlays, which bind none of the
//! three. A per-row flag would therefore be unused machinery. Terminal classes
//! are meant to diverge through a binding configuration, not here.
//!
//! A behavior is declared only in the contexts where it has an effect. Several
//! `AppState` methods guard on the focused field and do nothing elsewhere --
//! `insert_newline_if_message` outside `Message`, and
//! `autocomplete_active_recipient_field` outside `To` -- while the handlers
//! reach them from the whole screen mode. Declaring those inert rows would
//! preserve no behavior and would make generated help offer bindings that do
//! nothing, so they are omitted.

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use super::chord::Chord;
use super::context::BindingContext;

/// The draft a context's typed characters land in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextSink {
    Compose,
    Raww,
    PickerFilter,
}

impl TextSink {
    fn action(self, character: char) -> Action {
        match self {
            Self::Compose => Action::InsertComposeCharacter(character),
            Self::Raww => Action::InsertRawwCharacter(character),
            Self::PickerFilter => Action::AppendPickerFilterCharacter(character),
        }
    }

    /// What typing into this sink does, said without naming a character. The
    /// per-character actions carry the operator's own keystroke, so they have
    /// no description of their own worth presenting.
    fn describe(self) -> &'static str {
        match self {
            Self::Compose => "Insert into focused field",
            Self::Raww => "Insert into write input",
            Self::PickerFilter => "Filter focused column",
        }
    }
}

/// What a row produces. Most rows name one action outright; the typing rows
/// name where the character goes and take the character from the key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundAction {
    Fixed(Action),
    Typed(TextSink),
}

impl BoundAction {
    fn action_for(self, code: KeyCode) -> Option<Action> {
        match self {
            Self::Fixed(action) => Some(action),
            Self::Typed(sink) => match code {
                KeyCode::Char(character) => Some(sink.action(character)),
                _ => None,
            },
        }
    }

    /// What this row does, for presentation. This is also the key presentation
    /// groups rows by, which is why it is defined on `BoundAction` rather than
    /// on `Action`: the typing rows have no single action to describe.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Fixed(action) => action.describe(),
            Self::Typed(sink) => sink.describe(),
        }
    }
}

/// The heading a row is presented under. The variants are the sections the help
/// overlay already groups by, so generated presentation can reproduce its
/// layout rather than invent one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplaySection {
    Modes,
    Communication,
    Interaction,
    Picker,
    /// The overlay's own viewport controls. Filed apart from `Modes` -- which
    /// holds the chords that reach a surface -- because these reach nothing;
    /// they move what is drawn of the surface the operator is already on.
    Help,
}

/// One chord's binding within its context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContextRow {
    pub chord: Chord,
    pub action: BoundAction,
    pub section: DisplaySection,
}

/// Every row one context declares, in the order they are consulted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContextBindings {
    pub context: BindingContext,
    pub rows: &'static [ContextRow],
}

/// Resolves a key against one context's default rows, taking the first row that
/// matches. Declaration order is therefore the tiebreak: a row for a specific
/// character must precede its context's [`Chord::Text`] row, or typing that
/// character would reach the draft instead of its binding.
///
/// This reports the compiled **defaults**, and is the bottom tier of
/// [`super::EffectiveBindings`] rather than what dispatch or the help overlay
/// call directly: an operator's configuration resolves ahead of it, and answers
/// this question differently where it speaks. It remains the answer for a host
/// asking what a chord means before any configuration, and for the generated
/// usage guide, which has none to read.
pub fn default_binding(
    context: BindingContext,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    default_rows(context)
        .find(|row| row.chord.matches(code, modifiers))
        .and_then(|row| row.action.action_for(code))
}

/// Every row one context declares, in the order they are consulted.
pub(crate) fn default_rows(context: BindingContext) -> impl Iterator<Item = &'static ContextRow> {
    BINDINGS
        .iter()
        .find(move |group| group.context == context)
        .map(|group| group.rows.iter())
        .unwrap_or_default()
}

/// The heading one context's compiled rows file a behavior under.
///
/// A configured row carries a chord and a behavior and says nothing about where
/// to present it. It does not have to: a configuration may only bind a behavior
/// the context's compiled rows already declare, so the heading that behavior is
/// filed under in that context is always already decided. Reading it from here
/// is what keeps an operator's row in the section its behavior belongs to
/// rather than in one a configuration would otherwise have to name.
pub(crate) fn default_section(context: BindingContext, action: Action) -> Option<DisplaySection> {
    default_rows(context)
        .find(|row| row.action == BoundAction::Fixed(action))
        .map(|row| row.section)
}

const fn row(chord: Chord, action: Action, section: DisplaySection) -> ContextRow {
    ContextRow {
        chord,
        action: BoundAction::Fixed(action),
        section,
    }
}

const fn typing(sink: TextSink, section: DisplaySection) -> ContextRow {
    ContextRow {
        chord: Chord::Text,
        action: BoundAction::Typed(sink),
        section,
    }
}

/// One character carrying `Ctrl`, and nothing else. `Ctrl+Shift+J` is a
/// different keystroke and reaches this row no longer; a context wanting it
/// declares it.
const fn control(character: char) -> Chord {
    Chord::Key(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// One key with no modifier at all. Named for what it denotes rather than for
/// the handler condition it once reproduced.
const fn bare(code: KeyCode) -> Chord {
    Chord::Key(code, KeyModifiers::NONE)
}

const ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::NONE);
const SHIFT_ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::SHIFT);
const CONTROL_ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::CONTROL);

use Action as Act;
use DisplaySection as Section;

pub(crate) static BINDINGS: &[ContextBindings] = &[
    ContextBindings {
        context: BindingContext::Global,
        rows: GLOBAL,
    },
    ContextBindings {
        context: BindingContext::ComposeTo,
        rows: COMPOSE_TO,
    },
    ContextBindings {
        context: BindingContext::ComposeMessage,
        rows: COMPOSE_MESSAGE,
    },
    ContextBindings {
        context: BindingContext::InteractionWrite,
        rows: INTERACTION_WRITE,
    },
    ContextBindings {
        context: BindingContext::InteractionChoice,
        rows: INTERACTION_CHOICE,
    },
    ContextBindings {
        context: BindingContext::PickerBundles,
        rows: PICKER_BUNDLES,
    },
    ContextBindings {
        context: BindingContext::PickerSessions,
        rows: PICKER_SESSIONS,
    },
    ContextBindings {
        context: BindingContext::EventsOverlay,
        rows: EVENTS_OVERLAY,
    },
    ContextBindings {
        context: BindingContext::HelpOverlay,
        rows: HELP_OVERLAY,
    },
];

/// Rows that hold whichever surface is active. `handle_key` tests both of these
/// ahead of every overlay today, so they are declared here rather than repeated
/// in each context or left as an early return outside the table.
static GLOBAL: &[ContextRow] = &[
    row(control('c'), Act::Quit, Section::Modes),
    row(bare(KeyCode::F(1)), Act::ToggleHelpOverlay, Section::Modes),
];

static COMPOSE_TO: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(control('r'), Act::RefreshRecipients, Section::Modes),
    row(
        control('a'),
        Act::MoveToFieldCursorHome,
        Section::Communication,
    ),
    row(
        control('e'),
        Act::MoveToFieldCursorEnd,
        Section::Communication,
    ),
    row(control('u'), Act::ClearToField, Section::Communication),
    row(
        control(' '),
        Act::AutocompleteRecipient,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Tab),
        Act::CycleNextFocus,
        Section::Communication,
    ),
    row(
        bare(KeyCode::BackTab),
        Act::CyclePreviousFocus,
        Section::Communication,
    ),
    row(ENTER, Act::AcceptToCompletion, Section::Communication),
    row(SHIFT_ENTER, Act::AcceptToCompletion, Section::Communication),
    row(
        CONTROL_ENTER,
        Act::AcceptToCompletion,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Up),
        Act::MovePreviousToCompletion,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Down),
        Act::MoveNextToCompletion,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Left),
        Act::MoveToFieldCursorLeft,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Right),
        Act::MoveToFieldCursorRight,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Home),
        Act::MoveToFieldCursorHome,
        Section::Communication,
    ),
    row(
        bare(KeyCode::End),
        Act::MoveToFieldCursorEnd,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeleteComposeCharacter,
        Section::Communication,
    ),
    row(
        bare(KeyCode::PageUp),
        Act::ScrollChatHistoryPageUp,
        Section::Communication,
    ),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollChatHistoryPageDown,
        Section::Communication,
    ),
    typing(TextSink::Compose, Section::Communication),
];

static COMPOSE_MESSAGE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(control('r'), Act::RefreshRecipients, Section::Modes),
    row(
        control('a'),
        Act::MoveMessageCursorHome,
        Section::Communication,
    ),
    row(
        control('e'),
        Act::MoveMessageCursorEnd,
        Section::Communication,
    ),
    row(
        control('j'),
        Act::InsertMessageNewline,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Tab),
        Act::CycleNextFocus,
        Section::Communication,
    ),
    row(
        bare(KeyCode::BackTab),
        Act::CyclePreviousFocus,
        Section::Communication,
    ),
    row(ENTER, Act::SendMessage, Section::Communication),
    row(SHIFT_ENTER, Act::SendMessage, Section::Communication),
    row(CONTROL_ENTER, Act::SendMessage, Section::Communication),
    row(
        bare(KeyCode::Esc),
        Act::SnapChatHistoryToLatest,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Up),
        Act::MoveMessageCursorUp,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Down),
        Act::MoveMessageCursorDown,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Left),
        Act::MoveMessageCursorLeft,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Right),
        Act::MoveMessageCursorRight,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Home),
        Act::MoveMessageCursorHome,
        Section::Communication,
    ),
    row(
        bare(KeyCode::End),
        Act::MoveMessageCursorEnd,
        Section::Communication,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeleteComposeCharacter,
        Section::Communication,
    ),
    row(
        bare(KeyCode::PageUp),
        Act::ScrollChatHistoryPageUp,
        Section::Communication,
    ),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollChatHistoryPageDown,
        Section::Communication,
    ),
    typing(TextSink::Compose, Section::Communication),
];

static INTERACTION_WRITE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(control('r'), Act::RefreshRecipients, Section::Modes),
    row(control('j'), Act::InsertRawwNewline, Section::Interaction),
    row(ENTER, Act::DispatchRaww, Section::Interaction),
    row(SHIFT_ENTER, Act::DispatchRaww, Section::Interaction),
    row(CONTROL_ENTER, Act::DispatchRaww, Section::Interaction),
    row(
        bare(KeyCode::Left),
        Act::MoveRawwCursorLeft,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Right),
        Act::MoveRawwCursorRight,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Up),
        Act::NavigateInteractionUp,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Down),
        Act::NavigateInteractionDown,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Home),
        Act::MoveRawwCursorHome,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::End),
        Act::MoveRawwCursorEnd,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeleteRawwCharacter,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::PageUp),
        Act::ScrollInteractionSnapshotPageUp,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
        Section::Interaction,
    ),
    typing(TextSink::Raww, Section::Interaction),
];

static INTERACTION_CHOICE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(control('r'), Act::RefreshRecipients, Section::Modes),
    row(control('j'), Act::InsertRawwNewline, Section::Interaction),
    row(ENTER, Act::ResolveChoiceSelected, Section::Interaction),
    row(
        SHIFT_ENTER,
        Act::ResolveChoiceSelected,
        Section::Interaction,
    ),
    row(
        CONTROL_ENTER,
        Act::ResolveChoiceSelected,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Left),
        Act::MovePreviousChoiceRequest,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Right),
        Act::MoveNextChoiceRequest,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Up),
        Act::MovePreviousChoiceOption,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Down),
        Act::MoveNextChoiceOption,
        Section::Interaction,
    ),
    row(
        Chord::Char('c'),
        Act::ResolveChoiceCancelled,
        Section::Interaction,
    ),
    row(
        Chord::Char('C'),
        Act::ResolveChoiceCancelled,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeleteRawwCharacter,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::PageUp),
        Act::ScrollInteractionSnapshotPageUp,
        Section::Interaction,
    ),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
        Section::Interaction,
    ),
    typing(TextSink::Raww, Section::Interaction),
];

static PICKER_BUNDLES: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ClosePicker, Section::Picker),
    row(bare(KeyCode::F(2)), Act::ClosePicker, Section::Picker),
    row(bare(KeyCode::F(5)), Act::ClosePicker, Section::Picker),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(ENTER, Act::CommitPickerBundle, Section::Picker),
    row(SHIFT_ENTER, Act::CommitPickerBundle, Section::Picker),
    row(CONTROL_ENTER, Act::CommitPickerBundle, Section::Picker),
    row(bare(KeyCode::Tab), Act::TogglePickerFocus, Section::Picker),
    row(
        bare(KeyCode::BackTab),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(bare(KeyCode::Left), Act::TogglePickerFocus, Section::Picker),
    row(
        bare(KeyCode::Right),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Down),
        Act::MoveNextPickerSelection,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Up),
        Act::MovePreviousPickerSelection,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeletePickerFilterCharacter,
        Section::Picker,
    ),
    typing(TextSink::PickerFilter, Section::Picker),
];

static PICKER_SESSIONS: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ClosePicker, Section::Picker),
    row(bare(KeyCode::F(2)), Act::ClosePicker, Section::Picker),
    row(bare(KeyCode::F(5)), Act::ClosePicker, Section::Picker),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(ENTER, Act::CommitPickerSession, Section::Picker),
    row(SHIFT_ENTER, Act::CommitPickerSession, Section::Picker),
    row(CONTROL_ENTER, Act::CommitPickerSession, Section::Picker),
    row(bare(KeyCode::Tab), Act::TogglePickerFocus, Section::Picker),
    row(
        bare(KeyCode::BackTab),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(bare(KeyCode::Left), Act::TogglePickerFocus, Section::Picker),
    row(
        bare(KeyCode::Right),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Down),
        Act::MoveNextPickerSelection,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Up),
        Act::MovePreviousPickerSelection,
        Section::Picker,
    ),
    row(
        bare(KeyCode::Backspace),
        Act::DeletePickerFilterCharacter,
        Section::Picker,
    ),
    typing(TextSink::PickerFilter, Section::Picker),
];

static EVENTS_OVERLAY: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ToggleEventsOverlay, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
];

/// The overlay presents the whole surface, which is taller than a short
/// terminal can show, so it is drawn through a viewport. The six rows that move
/// that viewport are declared here like any other binding rather than authored
/// into the renderer: they were inert in this context before, so none of them
/// shadows a behavior, and no other context is touched.
static HELP_OVERLAY: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ToggleHelpOverlay, Section::Modes),
    row(bare(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(
        bare(KeyCode::F(3)),
        Act::ToggleEventsOverlay,
        Section::Modes,
    ),
    row(bare(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(bare(KeyCode::Up), Act::ScrollHelpUp, Section::Help),
    row(bare(KeyCode::Down), Act::ScrollHelpDown, Section::Help),
    row(bare(KeyCode::PageUp), Act::ScrollHelpPageUp, Section::Help),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollHelpPageDown,
        Section::Help,
    ),
    row(bare(KeyCode::Home), Act::ScrollHelpToStart, Section::Help),
    row(bare(KeyCode::End), Act::ScrollHelpToEnd, Section::Help),
];
