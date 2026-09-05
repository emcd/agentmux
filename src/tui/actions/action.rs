//! The named operator behaviors the workbench can perform.

use crate::runtime::error::RuntimeError;

use super::super::state::AppState;

/// Stands in for the typed character in [`Action::ALL`]'s data-carrying
/// entries. Never read: those entries exist so the list covers every variant,
/// and what distinguishes them is the variant rather than this value.
const PLACEHOLDER_CHARACTER: char = '\0';

/// Declares the behavior vocabulary once, so the enum, the list of every
/// behavior, and the operator-facing names cannot disagree.
///
/// Completeness is the reason this is a macro rather than three hand-kept
/// declarations. An exhaustive `match` forces a new variant to be *considered*,
/// but nothing forces it into a separate list, and a behavior missing from that
/// list would carry a name no lookup could find while every test that walks the
/// list still passed. Generating all three from one place removes the seam
/// entirely: a behavior exists only by appearing here.
///
/// A variant naming `None` carries the operator's typed character and is
/// therefore outside the configurable vocabulary; see
/// [`Action::carries_operator_input`].
macro_rules! declare_action_vocabulary {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident $(($payload:ty))? => $name:expr,
        )+
    ) => {
        /// Every operator-invocable workbench behavior, named independently of
        /// the key chord that reaches it.
        ///
        /// Resolving a chord to an `Action` is separable from applying one:
        /// applying requires no `KeyEvent`, so a host that owns its own event
        /// loop and its own bindings can drive the workbench by action alone.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Action {
            $(
                $(#[$meta])*
                $variant $(($payload))?,
            )+
        }

        impl Action {
            /// Every behavior in the vocabulary, so a caller can ask what the
            /// whole set contains rather than discovering members one binding
            /// at a time.
            ///
            /// Complete by construction: this and the enum are generated from
            /// one declaration, so a behavior cannot exist without appearing
            /// here. The variants carrying a typed character appear with a
            /// placeholder, since what distinguishes them is the variant rather
            /// than the character an operator happened to press.
            pub const ALL: &'static [Action] = &[
                $( Action::$variant $((declare_action_vocabulary!(@placeholder $payload)))? ),+
            ];

            /// This behavior's name in an operator's binding configuration,
            /// where it has one.
            ///
            /// Kebab-case, and deliberately not the variant identifier: a
            /// configuration is written by someone who has not read this
            /// source.
            ///
            /// Answers `None` for exactly the behaviors
            /// [`Action::carries_operator_input`] answers `true` for. The two
            /// are separate declarations that a test holds in agreement, rather
            /// than one derived from the other, so neither can be quietly
            /// widened.
            #[must_use]
            pub const fn configuration_name(self) -> Option<&'static str> {
                match self {
                    // A braced pattern matches a unit variant and a
                    // payload-carrying one alike, so the arm does not have to
                    // vary with the shape of the entry.
                    $( Self::$variant { .. } => $name, )+
                }
            }
        }
    };
    (@placeholder $payload:ty) => { PLACEHOLDER_CHARACTER };
}

declare_action_vocabulary! {
    /// Asks the event loop to shut down.
    Quit => Some("quit"),
    /// Opens the help overlay, or closes it when it is already open. Opening
    /// dismisses the picker and the events overlay.
    ToggleHelpOverlay => Some("toggle-help-overlay"),
    /// Opens the events overlay, or closes it when it is already open. Opening
    /// dismisses the picker and the help overlay.
    ToggleEventsOverlay => Some("toggle-events-overlay"),
    /// Moves the help overlay's viewport one row toward the start of its
    /// content.
    ///
    /// The overlay presents more than a short terminal can show, so its columns
    /// are drawn through a viewport. These six behaviors move it, and are the
    /// only actions in this vocabulary whose effect is confined to what is on
    /// screen.
    ScrollHelpUp => Some("scroll-help-up"),
    /// Moves the help overlay's viewport one row toward the end of its content.
    ScrollHelpDown => Some("scroll-help-down"),
    /// Moves the help overlay's viewport back by the height of its viewport.
    ScrollHelpPageUp => Some("scroll-help-page-up"),
    /// Moves the help overlay's viewport on by the height of its viewport.
    ScrollHelpPageDown => Some("scroll-help-page-down"),
    /// Returns the help overlay's viewport to the start of its content.
    ScrollHelpToStart => Some("scroll-help-to-start"),
    /// Moves the help overlay's viewport to the end of its content.
    ScrollHelpToEnd => Some("scroll-help-to-end"),
    /// Switches the active screen mode, dismissing whichever surface is open so
    /// the mode beneath it is the one that changes.
    ToggleMode => Some("toggle-mode"),
    /// Opens the unified picker on its session column.
    OpenPicker => Some("open-picker"),
    /// Opens the unified picker on its bundle column.
    OpenBundlePicker => Some("open-bundle-picker"),
    ClosePicker => Some("close-picker"),
    /// Re-enumerates recipients and cross-bundle completion candidates.
    RefreshRecipients => Some("refresh-recipients"),
    CycleNextFocus => Some("cycle-next-focus"),
    CyclePreviousFocus => Some("cycle-previous-focus"),
    AcceptToCompletion => Some("accept-to-completion"),
    SendMessage => Some("send-message"),
    InsertMessageNewline => Some("insert-message-newline"),
    AutocompleteRecipient => Some("autocomplete-recipient"),
    ClearToField => Some("clear-to-field"),
    MoveNextToCompletion => Some("move-next-to-completion"),
    MovePreviousToCompletion => Some("move-previous-to-completion"),
    MoveMessageCursorUp => Some("move-message-cursor-up"),
    MoveMessageCursorDown => Some("move-message-cursor-down"),
    MoveMessageCursorLeft => Some("move-message-cursor-left"),
    MoveMessageCursorRight => Some("move-message-cursor-right"),
    MoveMessageCursorHome => Some("move-message-cursor-home"),
    MoveMessageCursorEnd => Some("move-message-cursor-end"),
    MoveToFieldCursorLeft => Some("move-to-field-cursor-left"),
    MoveToFieldCursorRight => Some("move-to-field-cursor-right"),
    MoveToFieldCursorHome => Some("move-to-field-cursor-home"),
    MoveToFieldCursorEnd => Some("move-to-field-cursor-end"),
    DeleteComposeCharacter => Some("delete-compose-character"),
    InsertComposeCharacter(char) => None,
    SnapChatHistoryToLatest => Some("snap-chat-history-to-latest"),
    ScrollChatHistoryPageUp => Some("scroll-chat-history-page-up"),
    ScrollChatHistoryPageDown => Some("scroll-chat-history-page-down"),
    DispatchRaww => Some("dispatch-raww"),
    InsertRawwNewline => Some("insert-raww-newline"),
    DeleteRawwCharacter => Some("delete-raww-character"),
    InsertRawwCharacter(char) => None,
    MoveRawwCursorLeft => Some("move-raww-cursor-left"),
    MoveRawwCursorRight => Some("move-raww-cursor-right"),
    MoveRawwCursorHome => Some("move-raww-cursor-home"),
    MoveRawwCursorEnd => Some("move-raww-cursor-end"),
    /// Moves up within the interaction pane: through the write draft when one
    /// is present, through the look snapshot when it is not.
    NavigateInteractionUp => Some("navigate-interaction-up"),
    /// Moves down within the interaction pane, mirroring
    /// [`Action::NavigateInteractionUp`].
    NavigateInteractionDown => Some("navigate-interaction-down"),
    ScrollInteractionSnapshotPageUp => Some("scroll-interaction-snapshot-page-up"),
    ScrollInteractionSnapshotPageDown => Some("scroll-interaction-snapshot-page-down"),
    MoveNextChoiceRequest => Some("move-next-choice-request"),
    MovePreviousChoiceRequest => Some("move-previous-choice-request"),
    MoveNextChoiceOption => Some("move-next-choice-option"),
    MovePreviousChoiceOption => Some("move-previous-choice-option"),
    ResolveChoiceSelected => Some("resolve-choice-selected"),
    ResolveChoiceCancelled => Some("resolve-choice-cancelled"),
    TogglePickerFocus => Some("toggle-picker-focus"),
    MoveNextPickerSelection => Some("move-next-picker-selection"),
    MovePreviousPickerSelection => Some("move-previous-picker-selection"),
    CommitPickerBundle => Some("commit-picker-bundle"),
    /// Commits the selected session: inserted into the `To` field in
    /// Communication mode, opened as the interaction target in Interaction mode.
    CommitPickerSession => Some("commit-picker-session"),
    DeletePickerFilterCharacter => Some("delete-picker-filter-character"),
    AppendPickerFilterCharacter(char) => None,
}

impl Action {
    /// Whether performing this behavior needs the character the operator typed.
    ///
    /// These behaviors are constructed from a keystroke rather than named in
    /// advance, so a configuration row -- which supplies a chord and a name,
    /// never a character -- can neither denote nor build one. That is what puts
    /// them outside the configurable vocabulary.
    #[must_use]
    pub const fn carries_operator_input(self) -> bool {
        matches!(
            self,
            Self::InsertComposeCharacter(_)
                | Self::InsertRawwCharacter(_)
                | Self::AppendPickerFilterCharacter(_)
        )
    }

    /// The behavior a configuration name denotes, if any does.
    ///
    /// Derived by searching [`Action::ALL`] rather than by a second match, so
    /// the forward and reverse spellings cannot drift apart.
    #[must_use]
    pub fn from_configuration_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.configuration_name() == Some(name))
    }

    /// One line naming this behavior for an operator.
    ///
    /// Generated presentation groups rows by the behavior they reach, so this
    /// is the only place a binding's prose lives. Where two contexts bind the
    /// same chord to different behaviors -- `Enter` in the write pane and in
    /// the choice pane, `Ctrl+A` in `To` and in `Message` -- the wording says
    /// which, because presentation groups by action rather than by surface and
    /// the chord alone would not distinguish them.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Quit => "Quit from anywhere",
            Self::ToggleHelpOverlay => "Toggle help",
            Self::ToggleEventsOverlay => "Toggle events overlay",
            // Unqualified, because these are presented under a heading that
            // already names the overlay. Every other description that carries a
            // qualifier does so because its chord means something else on
            // another surface; these six chords are bound nowhere but here.
            Self::ScrollHelpUp => "Scroll up",
            Self::ScrollHelpDown => "Scroll down",
            Self::ScrollHelpPageUp => "Scroll up a page",
            Self::ScrollHelpPageDown => "Scroll down a page",
            Self::ScrollHelpToStart => "Jump to start",
            Self::ScrollHelpToEnd => "Jump to end",
            Self::ToggleMode => "Switch Communication / Interaction",
            Self::OpenPicker => "Open picker (sessions)",
            Self::OpenBundlePicker => "Open picker (bundles)",
            Self::ClosePicker => "Close picker",
            Self::RefreshRecipients => "Refresh recipients",
            Self::CycleNextFocus => "Focus next field",
            Self::CyclePreviousFocus => "Focus previous field",
            Self::AcceptToCompletion => "To: accept completion",
            Self::SendMessage => "Message: send",
            Self::InsertMessageNewline => "Message: insert newline",
            Self::AutocompleteRecipient => "To: trigger completion",
            Self::ClearToField => "To: clear field",
            Self::MoveNextToCompletion => "To: next completion",
            Self::MovePreviousToCompletion => "To: previous completion",
            Self::MoveMessageCursorUp => "Message: cursor up a line",
            Self::MoveMessageCursorDown => "Message: cursor down a line",
            Self::MoveMessageCursorLeft => "Message: cursor left",
            Self::MoveMessageCursorRight => "Message: cursor right",
            Self::MoveMessageCursorHome => "Message: line start",
            Self::MoveMessageCursorEnd => "Message: line end",
            Self::MoveToFieldCursorLeft => "To: cursor left",
            Self::MoveToFieldCursorRight => "To: cursor right",
            Self::MoveToFieldCursorHome => "To: field start",
            Self::MoveToFieldCursorEnd => "To: field end",
            Self::DeleteComposeCharacter => "Delete before cursor",
            Self::InsertComposeCharacter(_) => "Insert into focused field",
            Self::SnapChatHistoryToLatest => "Message: snap history",
            Self::ScrollChatHistoryPageUp => "Scroll chat history up",
            Self::ScrollChatHistoryPageDown => "Scroll chat history down",
            Self::DispatchRaww => "Write: dispatch to active target",
            Self::InsertRawwNewline => "Write: insert newline",
            Self::DeleteRawwCharacter => "Write: delete before cursor",
            Self::InsertRawwCharacter(_) => "Insert into write input",
            Self::MoveRawwCursorLeft => "Write: cursor left",
            Self::MoveRawwCursorRight => "Write: cursor right",
            Self::MoveRawwCursorHome => "Write: line start",
            Self::MoveRawwCursorEnd => "Write: line end",
            Self::NavigateInteractionUp => "Write: cursor up / scroll",
            Self::NavigateInteractionDown => "Write: cursor down / scroll",
            Self::ScrollInteractionSnapshotPageUp => "Scroll look snapshot up",
            Self::ScrollInteractionSnapshotPageDown => "Scroll look snapshot down",
            Self::MoveNextChoiceRequest => "Choice: next request",
            Self::MovePreviousChoiceRequest => "Choice: previous request",
            Self::MoveNextChoiceOption => "Choice: next ACP option",
            Self::MovePreviousChoiceOption => "Choice: previous ACP option",
            Self::ResolveChoiceSelected => "Choice: resolve selected option",
            Self::ResolveChoiceCancelled => "Choice: resolve as cancelled",
            Self::TogglePickerFocus => "Switch column",
            Self::MoveNextPickerSelection => "Next entry in column",
            Self::MovePreviousPickerSelection => "Previous entry in column",
            Self::CommitPickerBundle => "Bundle col: switch bundle",
            Self::CommitPickerSession => "Session col: insert or open look",
            Self::DeletePickerFilterCharacter => "Delete from column filter",
            Self::AppendPickerFilterCharacter(_) => "Filter focused column",
        }
    }

    /// Applies the behavior this action names.
    ///
    /// # Errors
    ///
    /// Returns the `RuntimeError` the underlying state operation produced, for
    /// the actions that reach the relay.
    pub(crate) fn apply(self, state: &mut AppState) -> Result<(), RuntimeError> {
        match self {
            Self::Quit => state.request_quit(),
            Self::ToggleHelpOverlay => state.toggle_help_overlay(),
            Self::ToggleEventsOverlay => state.toggle_events_overlay(),
            Self::ScrollHelpUp => state.scroll_help_overlay_up(),
            Self::ScrollHelpDown => state.scroll_help_overlay_down(),
            Self::ScrollHelpPageUp => state.scroll_help_overlay_page_up(),
            Self::ScrollHelpPageDown => state.scroll_help_overlay_page_down(),
            Self::ScrollHelpToStart => state.scroll_help_overlay_to_start(),
            Self::ScrollHelpToEnd => state.scroll_help_overlay_to_end(),
            Self::ToggleMode => {
                state.dismiss_surfaces();
                return state.toggle_mode();
            }
            Self::OpenPicker => state.open_picker(),
            Self::OpenBundlePicker => state.open_bundle_picker(),
            Self::ClosePicker => state.close_picker(),
            Self::RefreshRecipients => return state.refresh_recipients(),
            Self::CycleNextFocus => state.cycle_focus_forward(),
            Self::CyclePreviousFocus => state.cycle_focus_backward(),
            Self::AcceptToCompletion => {
                state.accept_active_to_completion();
            }
            Self::SendMessage => return state.send_message(),
            Self::InsertMessageNewline => state.insert_newline_if_message(),
            Self::AutocompleteRecipient => state.autocomplete_active_recipient_field(),
            Self::ClearToField => state.clear_to_field(),
            Self::MoveNextToCompletion => {
                state.move_to_completion_selection(1);
            }
            Self::MovePreviousToCompletion => {
                state.move_to_completion_selection(-1);
            }
            Self::MoveMessageCursorUp => state.move_message_cursor_up(),
            Self::MoveMessageCursorDown => state.move_message_cursor_down(),
            Self::MoveMessageCursorLeft => state.move_message_cursor_left(),
            Self::MoveMessageCursorRight => state.move_message_cursor_right(),
            Self::MoveMessageCursorHome => state.move_message_cursor_home(),
            Self::MoveMessageCursorEnd => state.move_message_cursor_end(),
            Self::MoveToFieldCursorLeft => state.move_to_field_cursor_left(),
            Self::MoveToFieldCursorRight => state.move_to_field_cursor_right(),
            Self::MoveToFieldCursorHome => state.move_to_field_cursor_home(),
            Self::MoveToFieldCursorEnd => state.move_to_field_cursor_end(),
            Self::DeleteComposeCharacter => state.backspace(),
            Self::InsertComposeCharacter(character) => state.insert_character(character),
            Self::SnapChatHistoryToLatest => state.snap_chat_history_to_latest(),
            Self::ScrollChatHistoryPageUp => state.scroll_chat_history_page_up(),
            Self::ScrollChatHistoryPageDown => state.scroll_chat_history_page_down(),
            Self::DispatchRaww => return state.dispatch_raww_from_interaction(),
            Self::InsertRawwNewline => state.insert_newline_in_raww(),
            Self::DeleteRawwCharacter => state.backspace_raww(),
            Self::InsertRawwCharacter(character) => state.insert_character_in_raww(character),
            Self::MoveRawwCursorLeft => state.move_raww_cursor_left(),
            Self::MoveRawwCursorRight => state.move_raww_cursor_right(),
            Self::MoveRawwCursorHome => state.move_raww_cursor_home(),
            Self::MoveRawwCursorEnd => state.move_raww_cursor_end(),
            Self::NavigateInteractionUp => state.navigate_interaction_up(),
            Self::NavigateInteractionDown => state.navigate_interaction_down(),
            Self::ScrollInteractionSnapshotPageUp => state.scroll_interaction_snapshot_page_up(),
            Self::ScrollInteractionSnapshotPageDown => {
                state.scroll_interaction_snapshot_page_down()
            }
            Self::MoveNextChoiceRequest => state.move_look_choice_request_selection(1),
            Self::MovePreviousChoiceRequest => state.move_look_choice_request_selection(-1),
            Self::MoveNextChoiceOption => state.move_look_choice_option_selection(1),
            Self::MovePreviousChoiceOption => state.move_look_choice_option_selection(-1),
            Self::ResolveChoiceSelected => return state.resolve_selected_look_choice_selected(),
            Self::ResolveChoiceCancelled => return state.resolve_selected_look_choice_cancelled(),
            Self::TogglePickerFocus => state.toggle_picker_focus(),
            Self::MoveNextPickerSelection => state.move_picker_selection(1),
            Self::MovePreviousPickerSelection => state.move_picker_selection(-1),
            Self::CommitPickerBundle => return state.commit_selected_picker_bundle(),
            Self::CommitPickerSession => return state.commit_selected_picker_session(),
            Self::DeletePickerFilterCharacter => state.picker_filter_backspace(),
            Self::AppendPickerFilterCharacter(character) => state.picker_filter_push(character),
        }
        Ok(())
    }
}
