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

use super::action::{Action, DisplaySection};
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
            Self::PickerFilter => {
                Action::AppendPickerFilterCharacter(character)
            },
        }
    }

    /// What typing into this sink is said under in presentation. The sink
    /// travels with the context rather than with a row, so this is what
    /// files the context's typing entry once the typing rows are gone.
    pub(crate) const fn section(self) -> DisplaySection {
        match self {
            Self::Compose => DisplaySection::Communication,
            Self::Raww => DisplaySection::Interaction,
            Self::PickerFilter => DisplaySection::Picker,
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

/// One chord's binding within its context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContextRow {
    pub chord: Chord,
    pub action: BoundAction,
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
pub(crate) fn default_rows(
    context: BindingContext,
) -> impl Iterator<Item = &'static ContextRow> {
    BINDINGS
        .iter()
        .find(move |group| group.context == context)
        .map(|group| group.rows.iter())
        .unwrap_or_default()
}

const fn row(chord: Chord, action: Action) -> ContextRow {
    ContextRow {
        chord,
        action: BoundAction::Fixed(action),
    }
}

const fn typing(sink: TextSink) -> ContextRow {
    ContextRow {
        chord: Chord::Text,
        action: BoundAction::Typed(sink),
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
    row(control('c'), Act::Quit),
    row(bare(KeyCode::F(1)), Act::ToggleHelpOverlay),
];

static COMPOSE_TO: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
    row(control('r'), Act::RefreshRecipients),
    row(control('a'), Act::MoveToFieldCursorHome),
    row(control('e'), Act::MoveToFieldCursorEnd),
    row(control('u'), Act::ClearToField),
    row(control(' '), Act::AutocompleteRecipient),
    row(bare(KeyCode::Tab), Act::CycleNextFocus),
    row(bare(KeyCode::BackTab), Act::CyclePreviousFocus),
    row(ENTER, Act::AcceptToCompletion),
    row(SHIFT_ENTER, Act::AcceptToCompletion),
    row(CONTROL_ENTER, Act::AcceptToCompletion),
    row(bare(KeyCode::Up), Act::MovePreviousToCompletion),
    row(bare(KeyCode::Down), Act::MoveNextToCompletion),
    row(bare(KeyCode::Left), Act::MoveToFieldCursorLeft),
    row(bare(KeyCode::Right), Act::MoveToFieldCursorRight),
    row(bare(KeyCode::Home), Act::MoveToFieldCursorHome),
    row(bare(KeyCode::End), Act::MoveToFieldCursorEnd),
    row(bare(KeyCode::Backspace), Act::DeleteComposeCharacter),
    row(bare(KeyCode::PageUp), Act::ScrollChatHistoryPageUp),
    row(bare(KeyCode::PageDown), Act::ScrollChatHistoryPageDown),
    typing(TextSink::Compose),
];

static COMPOSE_MESSAGE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
    row(control('r'), Act::RefreshRecipients),
    row(control('a'), Act::MoveMessageCursorHome),
    row(control('e'), Act::MoveMessageCursorEnd),
    row(control('j'), Act::InsertMessageNewline),
    row(bare(KeyCode::Tab), Act::CycleNextFocus),
    row(bare(KeyCode::BackTab), Act::CyclePreviousFocus),
    row(ENTER, Act::SendMessage),
    row(SHIFT_ENTER, Act::SendMessage),
    row(CONTROL_ENTER, Act::SendMessage),
    row(bare(KeyCode::Esc), Act::SnapChatHistoryToLatest),
    row(bare(KeyCode::Up), Act::MoveMessageCursorUp),
    row(bare(KeyCode::Down), Act::MoveMessageCursorDown),
    row(bare(KeyCode::Left), Act::MoveMessageCursorLeft),
    row(bare(KeyCode::Right), Act::MoveMessageCursorRight),
    row(bare(KeyCode::Home), Act::MoveMessageCursorHome),
    row(bare(KeyCode::End), Act::MoveMessageCursorEnd),
    row(bare(KeyCode::Backspace), Act::DeleteComposeCharacter),
    row(bare(KeyCode::PageUp), Act::ScrollChatHistoryPageUp),
    row(bare(KeyCode::PageDown), Act::ScrollChatHistoryPageDown),
    typing(TextSink::Compose),
];

static INTERACTION_WRITE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
    row(control('r'), Act::RefreshRecipients),
    row(control('j'), Act::InsertRawwNewline),
    row(ENTER, Act::DispatchRaww),
    row(SHIFT_ENTER, Act::DispatchRaww),
    row(CONTROL_ENTER, Act::DispatchRaww),
    row(bare(KeyCode::Left), Act::MoveRawwCursorLeft),
    row(bare(KeyCode::Right), Act::MoveRawwCursorRight),
    row(bare(KeyCode::Up), Act::NavigateInteractionUp),
    row(bare(KeyCode::Down), Act::NavigateInteractionDown),
    row(bare(KeyCode::Home), Act::MoveRawwCursorHome),
    row(bare(KeyCode::End), Act::MoveRawwCursorEnd),
    row(bare(KeyCode::Backspace), Act::DeleteRawwCharacter),
    row(bare(KeyCode::PageUp), Act::ScrollInteractionSnapshotPageUp),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
    ),
    typing(TextSink::Raww),
];

static INTERACTION_CHOICE: &[ContextRow] = &[
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
    row(control('r'), Act::RefreshRecipients),
    row(control('j'), Act::InsertRawwNewline),
    row(ENTER, Act::ResolveChoiceSelected),
    row(SHIFT_ENTER, Act::ResolveChoiceSelected),
    row(CONTROL_ENTER, Act::ResolveChoiceSelected),
    row(bare(KeyCode::Left), Act::MovePreviousChoiceRequest),
    row(bare(KeyCode::Right), Act::MoveNextChoiceRequest),
    row(bare(KeyCode::Up), Act::MovePreviousChoiceOption),
    row(bare(KeyCode::Down), Act::MoveNextChoiceOption),
    row(Chord::Char('c'), Act::ResolveChoiceCancelled),
    row(Chord::Char('C'), Act::ResolveChoiceCancelled),
    row(bare(KeyCode::Backspace), Act::DeleteRawwCharacter),
    row(bare(KeyCode::PageUp), Act::ScrollInteractionSnapshotPageUp),
    row(
        bare(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
    ),
    typing(TextSink::Raww),
];

static PICKER_BUNDLES: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ClosePicker),
    row(bare(KeyCode::F(2)), Act::ClosePicker),
    row(bare(KeyCode::F(5)), Act::ClosePicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(ENTER, Act::CommitPickerBundle),
    row(SHIFT_ENTER, Act::CommitPickerBundle),
    row(CONTROL_ENTER, Act::CommitPickerBundle),
    row(bare(KeyCode::Tab), Act::TogglePickerFocus),
    row(bare(KeyCode::BackTab), Act::TogglePickerFocus),
    row(bare(KeyCode::Left), Act::TogglePickerFocus),
    row(bare(KeyCode::Right), Act::TogglePickerFocus),
    row(bare(KeyCode::Down), Act::MoveNextPickerSelection),
    row(bare(KeyCode::Up), Act::MovePreviousPickerSelection),
    row(bare(KeyCode::Backspace), Act::DeletePickerFilterCharacter),
    typing(TextSink::PickerFilter),
];

static PICKER_SESSIONS: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ClosePicker),
    row(bare(KeyCode::F(2)), Act::ClosePicker),
    row(bare(KeyCode::F(5)), Act::ClosePicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(ENTER, Act::CommitPickerSession),
    row(SHIFT_ENTER, Act::CommitPickerSession),
    row(CONTROL_ENTER, Act::CommitPickerSession),
    row(bare(KeyCode::Tab), Act::TogglePickerFocus),
    row(bare(KeyCode::BackTab), Act::TogglePickerFocus),
    row(bare(KeyCode::Left), Act::TogglePickerFocus),
    row(bare(KeyCode::Right), Act::TogglePickerFocus),
    row(bare(KeyCode::Down), Act::MoveNextPickerSelection),
    row(bare(KeyCode::Up), Act::MovePreviousPickerSelection),
    row(bare(KeyCode::Backspace), Act::DeletePickerFilterCharacter),
    typing(TextSink::PickerFilter),
];

static EVENTS_OVERLAY: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
];

/// The overlay presents the whole surface, which is taller than a short
/// terminal can show, so it is drawn through a viewport. The six rows that move
/// that viewport are declared here like any other binding rather than authored
/// into the renderer: they were inert in this context before, so none of them
/// shadows a behavior, and no other context is touched.
static HELP_OVERLAY: &[ContextRow] = &[
    row(bare(KeyCode::Esc), Act::ToggleHelpOverlay),
    row(bare(KeyCode::F(2)), Act::OpenPicker),
    row(bare(KeyCode::F(3)), Act::ToggleEventsOverlay),
    row(bare(KeyCode::F(4)), Act::ToggleMode),
    row(bare(KeyCode::F(5)), Act::OpenBundlePicker),
    row(bare(KeyCode::Up), Act::ScrollHelpUp),
    row(bare(KeyCode::Down), Act::ScrollHelpDown),
    row(bare(KeyCode::PageUp), Act::ScrollHelpPageUp),
    row(bare(KeyCode::PageDown), Act::ScrollHelpPageDown),
    row(bare(KeyCode::Home), Act::ScrollHelpToStart),
    row(bare(KeyCode::End), Act::ScrollHelpToEnd),
];
