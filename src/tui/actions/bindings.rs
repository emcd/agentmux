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
use super::context::BindingContext;

/// The pattern a row matches an incoming key against. Each variant mirrors a
/// condition shape the handlers in `../input.rs` use today, so the table can
/// reproduce them without narrowing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Chord {
    /// One key with an exact modifier set. Used where a modifier is part of the
    /// chord's identity, so `Enter`, `Shift+Enter`, and `Ctrl+Enter` stay
    /// distinguishable rows.
    Key(KeyCode, KeyModifiers),
    /// One key whatever modifiers accompany it, mirroring the handler arms that
    /// test only the key code.
    AnyModifiers(KeyCode),
    /// One character carrying `Ctrl`, whatever else accompanies it. The control
    /// blocks in `../input.rs` test `modifiers.contains(CONTROL)`, so
    /// `Ctrl+Shift+J` reaches the same behavior as `Ctrl+J` today.
    Control(char),
    /// One character typed bare or with `Shift`.
    Char(char),
    /// Any character typed bare or with `Shift`. The character is carried into
    /// the action rather than into the row, since a row per character is not a
    /// thing a table can hold.
    Text,
}

impl Chord {
    /// How this chord is written for an operator. Presentation folds several
    /// rows onto one line, so a chord that renders the same as one already on
    /// the line disappears into it -- which is what keeps the `Enter` fallback
    /// row from printing a second, identical "Enter".
    pub(crate) fn display(self) -> String {
        match self {
            Self::Key(code, modifiers) => {
                let mut text = String::new();
                if modifiers.contains(KeyModifiers::CONTROL) {
                    text.push_str("Ctrl+");
                }
                if modifiers.contains(KeyModifiers::ALT) {
                    text.push_str("Alt+");
                }
                if modifiers.contains(KeyModifiers::SHIFT) {
                    text.push_str("Shift+");
                }
                text.push_str(&key_code_display(code));
                text
            }
            Self::AnyModifiers(code) => key_code_display(code),
            // Control chords are conventionally written with a capital, and
            // the table stores them lowercase because that is the character
            // the terminal reports. A literal typed character is not
            // capitalised: `c` and `C` are separate rows on purpose.
            Self::Control(character) => {
                format!("Ctrl+{}", character_display(character.to_ascii_uppercase()))
            }
            Self::Char(character) => character_display(character),
            Self::Text => "Type".to_string(),
        }
    }

    pub(super) fn matches(self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match self {
            Self::Key(row_code, row_modifiers) => code == row_code && modifiers == row_modifiers,
            Self::AnyModifiers(row_code) => code == row_code,
            Self::Control(character) => {
                code == KeyCode::Char(character) && modifiers.contains(KeyModifiers::CONTROL)
            }
            Self::Char(character) => code == KeyCode::Char(character) && is_typed(modifiers),
            Self::Text => matches!(code, KeyCode::Char(_)) && is_typed(modifiers),
        }
    }
}

/// Whether a character reached the terminal as ordinary typing rather than as
/// part of a modified chord.
fn is_typed(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn key_code_display(code: KeyCode) -> String {
    match code {
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::F(number) => format!("F{number}"),
        KeyCode::Char(character) => character_display(character),
        other => format!("{other:?}"),
    }
}

fn character_display(character: char) -> String {
    match character {
        ' ' => "Space".to_string(),
        other => other.to_string(),
    }
}

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
/// This reports the compiled **defaults**. It is the read side of the table for
/// dispatch, presentation, and any host asking what a chord means on a surface;
/// it is not a claim that the chord is fixed, since operator-configured
/// bindings are the intended successor to this table.
pub fn default_binding(
    context: BindingContext,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|group| group.context == context)?
        .rows
        .iter()
        .find(|row| row.chord.matches(code, modifiers))
        .and_then(|row| row.action.action_for(code))
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

const fn control(character: char) -> Chord {
    Chord::Control(character)
}

const fn any(code: KeyCode) -> Chord {
    Chord::AnyModifiers(code)
}

const ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::NONE);
const SHIFT_ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::SHIFT);
const CONTROL_ENTER: Chord = Chord::Key(KeyCode::Enter, KeyModifiers::CONTROL);
/// Any other modifier set on `Enter`, for the contexts whose handler arm tests
/// only `KeyCode::Enter`. Declared after the three explicit rows, which own the
/// modifier sets the capability-neutrality contract governs; this one exists so
/// `Alt+Enter` keeps reaching the action it reaches today. Compose does not
/// carry it, because its arm guards on `modifiers.is_empty()`.
const OTHER_ENTER: Chord = Chord::AnyModifiers(KeyCode::Enter);

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
    row(any(KeyCode::F(1)), Act::ToggleHelpOverlay, Section::Modes),
];

static COMPOSE_TO: &[ContextRow] = &[
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
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
        any(KeyCode::Tab),
        Act::CycleNextFocus,
        Section::Communication,
    ),
    row(
        any(KeyCode::BackTab),
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
        any(KeyCode::Up),
        Act::MovePreviousToCompletion,
        Section::Communication,
    ),
    row(
        any(KeyCode::Down),
        Act::MoveNextToCompletion,
        Section::Communication,
    ),
    row(
        any(KeyCode::Left),
        Act::MoveToFieldCursorLeft,
        Section::Communication,
    ),
    row(
        any(KeyCode::Right),
        Act::MoveToFieldCursorRight,
        Section::Communication,
    ),
    row(
        any(KeyCode::Home),
        Act::MoveToFieldCursorHome,
        Section::Communication,
    ),
    row(
        any(KeyCode::End),
        Act::MoveToFieldCursorEnd,
        Section::Communication,
    ),
    row(
        any(KeyCode::Backspace),
        Act::DeleteComposeCharacter,
        Section::Communication,
    ),
    row(
        any(KeyCode::PageUp),
        Act::ScrollChatHistoryPageUp,
        Section::Communication,
    ),
    row(
        any(KeyCode::PageDown),
        Act::ScrollChatHistoryPageDown,
        Section::Communication,
    ),
    typing(TextSink::Compose, Section::Communication),
];

static COMPOSE_MESSAGE: &[ContextRow] = &[
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
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
        any(KeyCode::Tab),
        Act::CycleNextFocus,
        Section::Communication,
    ),
    row(
        any(KeyCode::BackTab),
        Act::CyclePreviousFocus,
        Section::Communication,
    ),
    row(ENTER, Act::SendMessage, Section::Communication),
    row(SHIFT_ENTER, Act::SendMessage, Section::Communication),
    row(CONTROL_ENTER, Act::SendMessage, Section::Communication),
    row(
        any(KeyCode::Esc),
        Act::SnapChatHistoryToLatest,
        Section::Communication,
    ),
    row(
        any(KeyCode::Up),
        Act::MoveMessageCursorUp,
        Section::Communication,
    ),
    row(
        any(KeyCode::Down),
        Act::MoveMessageCursorDown,
        Section::Communication,
    ),
    row(
        any(KeyCode::Left),
        Act::MoveMessageCursorLeft,
        Section::Communication,
    ),
    row(
        any(KeyCode::Right),
        Act::MoveMessageCursorRight,
        Section::Communication,
    ),
    row(
        any(KeyCode::Home),
        Act::MoveMessageCursorHome,
        Section::Communication,
    ),
    row(
        any(KeyCode::End),
        Act::MoveMessageCursorEnd,
        Section::Communication,
    ),
    row(
        any(KeyCode::Backspace),
        Act::DeleteComposeCharacter,
        Section::Communication,
    ),
    row(
        any(KeyCode::PageUp),
        Act::ScrollChatHistoryPageUp,
        Section::Communication,
    ),
    row(
        any(KeyCode::PageDown),
        Act::ScrollChatHistoryPageDown,
        Section::Communication,
    ),
    typing(TextSink::Compose, Section::Communication),
];

static INTERACTION_WRITE: &[ContextRow] = &[
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(control('r'), Act::RefreshRecipients, Section::Modes),
    row(control('j'), Act::InsertRawwNewline, Section::Interaction),
    row(ENTER, Act::DispatchRaww, Section::Interaction),
    row(SHIFT_ENTER, Act::DispatchRaww, Section::Interaction),
    row(CONTROL_ENTER, Act::DispatchRaww, Section::Interaction),
    row(OTHER_ENTER, Act::DispatchRaww, Section::Interaction),
    row(
        any(KeyCode::Left),
        Act::MoveRawwCursorLeft,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Right),
        Act::MoveRawwCursorRight,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Up),
        Act::NavigateInteractionUp,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Down),
        Act::NavigateInteractionDown,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Home),
        Act::MoveRawwCursorHome,
        Section::Interaction,
    ),
    row(
        any(KeyCode::End),
        Act::MoveRawwCursorEnd,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Backspace),
        Act::DeleteRawwCharacter,
        Section::Interaction,
    ),
    row(
        any(KeyCode::PageUp),
        Act::ScrollInteractionSnapshotPageUp,
        Section::Interaction,
    ),
    row(
        any(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
        Section::Interaction,
    ),
    typing(TextSink::Raww, Section::Interaction),
];

static INTERACTION_CHOICE: &[ContextRow] = &[
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
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
        OTHER_ENTER,
        Act::ResolveChoiceSelected,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Left),
        Act::MovePreviousChoiceRequest,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Right),
        Act::MoveNextChoiceRequest,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Up),
        Act::MovePreviousChoiceOption,
        Section::Interaction,
    ),
    row(
        any(KeyCode::Down),
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
        any(KeyCode::Backspace),
        Act::DeleteRawwCharacter,
        Section::Interaction,
    ),
    row(
        any(KeyCode::PageUp),
        Act::ScrollInteractionSnapshotPageUp,
        Section::Interaction,
    ),
    row(
        any(KeyCode::PageDown),
        Act::ScrollInteractionSnapshotPageDown,
        Section::Interaction,
    ),
    typing(TextSink::Raww, Section::Interaction),
];

static PICKER_BUNDLES: &[ContextRow] = &[
    row(any(KeyCode::Esc), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(2)), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(5)), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(ENTER, Act::CommitPickerBundle, Section::Picker),
    row(SHIFT_ENTER, Act::CommitPickerBundle, Section::Picker),
    row(CONTROL_ENTER, Act::CommitPickerBundle, Section::Picker),
    row(OTHER_ENTER, Act::CommitPickerBundle, Section::Picker),
    row(any(KeyCode::Tab), Act::TogglePickerFocus, Section::Picker),
    row(
        any(KeyCode::BackTab),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(any(KeyCode::Left), Act::TogglePickerFocus, Section::Picker),
    row(any(KeyCode::Right), Act::TogglePickerFocus, Section::Picker),
    row(
        any(KeyCode::Down),
        Act::MoveNextPickerSelection,
        Section::Picker,
    ),
    row(
        any(KeyCode::Up),
        Act::MovePreviousPickerSelection,
        Section::Picker,
    ),
    row(
        any(KeyCode::Backspace),
        Act::DeletePickerFilterCharacter,
        Section::Picker,
    ),
    typing(TextSink::PickerFilter, Section::Picker),
];

static PICKER_SESSIONS: &[ContextRow] = &[
    row(any(KeyCode::Esc), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(2)), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(5)), Act::ClosePicker, Section::Picker),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(ENTER, Act::CommitPickerSession, Section::Picker),
    row(SHIFT_ENTER, Act::CommitPickerSession, Section::Picker),
    row(CONTROL_ENTER, Act::CommitPickerSession, Section::Picker),
    row(OTHER_ENTER, Act::CommitPickerSession, Section::Picker),
    row(any(KeyCode::Tab), Act::TogglePickerFocus, Section::Picker),
    row(
        any(KeyCode::BackTab),
        Act::TogglePickerFocus,
        Section::Picker,
    ),
    row(any(KeyCode::Left), Act::TogglePickerFocus, Section::Picker),
    row(any(KeyCode::Right), Act::TogglePickerFocus, Section::Picker),
    row(
        any(KeyCode::Down),
        Act::MoveNextPickerSelection,
        Section::Picker,
    ),
    row(
        any(KeyCode::Up),
        Act::MovePreviousPickerSelection,
        Section::Picker,
    ),
    row(
        any(KeyCode::Backspace),
        Act::DeletePickerFilterCharacter,
        Section::Picker,
    ),
    typing(TextSink::PickerFilter, Section::Picker),
];

static EVENTS_OVERLAY: &[ContextRow] = &[
    row(any(KeyCode::Esc), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
];

/// The overlay presents the whole surface, which is taller than a short
/// terminal can show, so it is drawn through a viewport. The six rows that move
/// that viewport are declared here like any other binding rather than authored
/// into the renderer: they were inert in this context before, so none of them
/// shadows a behavior, and no other context is touched.
static HELP_OVERLAY: &[ContextRow] = &[
    row(any(KeyCode::Esc), Act::ToggleHelpOverlay, Section::Modes),
    row(any(KeyCode::F(2)), Act::OpenPicker, Section::Modes),
    row(any(KeyCode::F(3)), Act::ToggleEventsOverlay, Section::Modes),
    row(any(KeyCode::F(4)), Act::ToggleMode, Section::Modes),
    row(any(KeyCode::F(5)), Act::OpenBundlePicker, Section::Modes),
    row(any(KeyCode::Up), Act::ScrollHelpUp, Section::Help),
    row(any(KeyCode::Down), Act::ScrollHelpDown, Section::Help),
    row(any(KeyCode::PageUp), Act::ScrollHelpPageUp, Section::Help),
    row(
        any(KeyCode::PageDown),
        Act::ScrollHelpPageDown,
        Section::Help,
    ),
    row(any(KeyCode::Home), Act::ScrollHelpToStart, Section::Help),
    row(any(KeyCode::End), Act::ScrollHelpToEnd, Section::Help),
];
