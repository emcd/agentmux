//! The named operator behaviors the workbench can perform.

use crate::runtime::error::RuntimeError;

use super::super::state::AppState;

/// Every operator-invocable workbench behavior, named independently of the key
/// chord that reaches it.
///
/// Resolving a chord to an `Action` is separable from applying one: applying
/// requires no `KeyEvent`, so a host that owns its own event loop and its own
/// bindings can drive the workbench by action alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Asks the event loop to shut down.
    Quit,
    /// Opens the help overlay, or closes it when it is already open. Opening
    /// dismisses the picker and the events overlay.
    ToggleHelpOverlay,
    /// Opens the events overlay, or closes it when it is already open. Opening
    /// dismisses the picker and the help overlay.
    ToggleEventsOverlay,
    /// Moves the help overlay's viewport one row toward the start of its
    /// content.
    ///
    /// The overlay presents more than a short terminal can show, so its columns
    /// are drawn through a viewport. These six behaviors move it, and are the
    /// only actions in this vocabulary whose effect is confined to what is on
    /// screen.
    ScrollHelpUp,
    /// Moves the help overlay's viewport one row toward the end of its content.
    ScrollHelpDown,
    /// Moves the help overlay's viewport back by the height of its viewport.
    ScrollHelpPageUp,
    /// Moves the help overlay's viewport on by the height of its viewport.
    ScrollHelpPageDown,
    /// Returns the help overlay's viewport to the start of its content.
    ScrollHelpToStart,
    /// Moves the help overlay's viewport to the end of its content.
    ScrollHelpToEnd,
    /// Switches the active screen mode, dismissing whichever surface is open so
    /// the mode beneath it is the one that changes.
    ToggleMode,
    /// Opens the unified picker on its session column.
    OpenPicker,
    /// Opens the unified picker on its bundle column.
    OpenBundlePicker,
    ClosePicker,
    /// Re-enumerates recipients and cross-bundle completion candidates.
    RefreshRecipients,
    CycleNextFocus,
    CyclePreviousFocus,
    AcceptToCompletion,
    SendMessage,
    InsertMessageNewline,
    AutocompleteRecipient,
    ClearToField,
    MoveNextToCompletion,
    MovePreviousToCompletion,
    MoveMessageCursorUp,
    MoveMessageCursorDown,
    MoveMessageCursorLeft,
    MoveMessageCursorRight,
    MoveMessageCursorHome,
    MoveMessageCursorEnd,
    MoveToFieldCursorLeft,
    MoveToFieldCursorRight,
    MoveToFieldCursorHome,
    MoveToFieldCursorEnd,
    DeleteComposeCharacter,
    InsertComposeCharacter(char),
    SnapChatHistoryToLatest,
    ScrollChatHistoryPageUp,
    ScrollChatHistoryPageDown,
    DispatchRaww,
    InsertRawwNewline,
    DeleteRawwCharacter,
    InsertRawwCharacter(char),
    MoveRawwCursorLeft,
    MoveRawwCursorRight,
    MoveRawwCursorHome,
    MoveRawwCursorEnd,
    /// Moves up within the interaction pane: through the write draft when one
    /// is present, through the look snapshot when it is not.
    NavigateInteractionUp,
    /// Moves down within the interaction pane, mirroring
    /// [`Action::NavigateInteractionUp`].
    NavigateInteractionDown,
    ScrollInteractionSnapshotPageUp,
    ScrollInteractionSnapshotPageDown,
    MoveNextChoiceRequest,
    MovePreviousChoiceRequest,
    MoveNextChoiceOption,
    MovePreviousChoiceOption,
    ResolveChoiceSelected,
    ResolveChoiceCancelled,
    TogglePickerFocus,
    MoveNextPickerSelection,
    MovePreviousPickerSelection,
    CommitPickerBundle,
    /// Commits the selected session: inserted into the `To` field in
    /// Communication mode, opened as the interaction target in Interaction mode.
    CommitPickerSession,
    DeletePickerFilterCharacter,
    AppendPickerFilterCharacter(char),
}

impl Action {
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
