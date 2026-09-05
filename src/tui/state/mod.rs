use std::{
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
};

use ratatui::widgets::ListState;

use crate::{
    relay::{RelayError, RelayStreamSession},
    runtime::error::RuntimeError,
    transports::StructuredEntry,
};

use super::target::{
    ToCompletionState, append_recipient_token, current_recipient_token_context,
    matching_recipient_candidates, merge_tui_targets, sender_bound_bundle,
};

mod compose;
mod history;
mod relay;

use super::actions::{BindingConfiguration, CapabilityClass, EffectiveBindings};
use super::keyboard::KeyboardEnhancement;
use super::status::BundleStatusDisplay;

const STATUS_HISTORY_MAXIMUM: usize = 6;
const EVENT_HISTORY_MAXIMUM: usize = 64;
const CHAT_HISTORY_MAXIMUM: usize = 256;
const SEEN_STREAM_IDS_MAXIMUM: usize = 1024;

#[derive(Clone, Debug)]
pub(crate) enum ChatHistoryDirection {
    Outgoing,
    Incoming,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatHistoryEntry {
    pub direction: ChatHistoryDirection,
    pub peer_session: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LookSnapshotFormat {
    Lines,
    StructuredEntriesV1,
}

#[derive(Clone, Debug)]
pub struct TuiLaunchOptions {
    pub namespace: String,
    pub sender_session: String,
    pub relay_socket: PathBuf,
    pub look_lines: Option<u64>,
    pub available_bundles: Vec<String>,
    /// The operator's validated binding group, absent where their `ui.toml`
    /// declares none or does not exist.
    ///
    /// A launch option rather than something set afterwards, because the
    /// effective table is built once and never changes for the life of a run.
    /// Handing it in here is what makes it impossible to start a workbench that
    /// silently ignored a configuration the operator wrote.
    pub bindings: Option<BindingConfiguration>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FocusField {
    #[default]
    To,
    Message,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ScreenMode {
    #[default]
    Communication,
    Interaction,
}

/// Which column of the unified bundle+session picker currently has keyboard
/// focus. The filter and Up/Down navigation apply to the focused column; the
/// other column shows its full (unfiltered) list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerColumn {
    Bundles,
    #[default]
    Sessions,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusEntry {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Recipient {
    pub session_name: String,
    pub display_name: Option<String>,
    pub ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingChoiceEntry {
    pub choice_request_id: String,
    pub message_id: Option<String>,
    pub target_session: Option<String>,
    pub requested_kind: Option<String>,
    pub requested_details: Option<serde_json::Value>,
    pub enqueued_at: Option<String>,
    pub options: Vec<PendingChoiceOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingChoiceOption {
    pub option_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug)]
pub(crate) struct AppState {
    pub namespace: String,
    pub sender_session: String,
    relay_socket: PathBuf,
    relay_stream: RelayStreamSession,
    look_lines: Option<u64>,
    pub recipients: Vec<Recipient>,
    /// Relay-wide To-field completion candidates as full `session@bundle`
    /// principal ids, spanning every available bundle other than the active
    /// `namespace` (whose sessions are already offered via `recipients`, which
    /// the relay returns as canonical `session@bundle` ids too). Refreshed
    /// eagerly on the same cadence as `recipients`; kept separate so the
    /// recipient picker stays scoped to the active bundle.
    pub cross_bundle_candidates: Vec<String>,
    pub bundle_status: Option<BundleStatusDisplay>,
    pub last_selected_recipient: Option<String>,
    pub available_bundles: Vec<String>,
    pub picker_open: bool,
    pub picker_focus: PickerColumn,
    pub picker_filter: String,
    pub events_overlay_open: bool,
    pub help_overlay_open: bool,
    /// Rows the help overlay's columns are drawn past.
    ///
    /// Counted from the start of the content, unlike `chat_history_scroll`,
    /// which counts back from the newest entry. Help is a document read
    /// downward and has no newest end to anchor to.
    help_overlay_scroll: usize,
    /// Viewport bounds for that offset, published by the renderer.
    ///
    /// How many rows a column occupies is a function of wrapping at the width
    /// it was drawn at, which nothing outside the renderer knows. They are
    /// therefore a frame behind; the overlay is drawn on the frame that opens
    /// it, so no operator-reachable sequence reads them before they are set.
    help_overlay_page_rows: usize,
    help_overlay_maximum_scroll: usize,
    pub picker_session_state: ListState,
    pub picker_bundle_state: ListState,
    pub mode: ScreenMode,
    pub focus: FocusField,
    /// Startup keyboard-enhancement probe outcome. Defaults to `Unsupported` —
    /// the same reporting a terminal without the protocol gives — and `run`
    /// overwrites it with the real probe result before the event loop starts.
    ///
    /// Written through [`AppState::set_keyboard_enhancement`], because the
    /// effective table is built from it and a bare assignment would leave the
    /// two disagreeing.
    keyboard_enhancement: KeyboardEnhancement,
    /// What the operator configured, kept so the effective table can be rebuilt
    /// once the probe outcome is known.
    binding_configuration: Option<BindingConfiguration>,
    /// The bindings in force: what the operator configured over what ships.
    /// Dispatch resolves against this, and generated presentation reads it.
    pub bindings: EffectiveBindings,
    pub to_field: String,
    to_cursor_index: usize,
    pub message_field: String,
    message_cursor_index: usize,
    message_cursor_preferred_column: Option<usize>,
    pub raww_draft: String,
    pub(crate) raww_cursor_index: usize,
    raww_cursor_preferred_column: Option<usize>,
    pub look_target: Option<String>,
    pub look_captured_at: Option<String>,
    pub look_snapshot_format: Option<LookSnapshotFormat>,
    pub look_snapshot_lines: Vec<String>,
    pub look_snapshot_entries: Vec<StructuredEntry>,
    pub look_overlay_scroll: usize,
    pub(crate) look_choice_request_index: usize,
    pub(crate) look_choice_option_index: usize,
    pub status_history: VecDeque<StatusEntry>,
    pub event_history: VecDeque<String>,
    pub pending_choices: Vec<PendingChoiceEntry>,
    pub pending_choices_state: ListState,
    pub chat_history: VecDeque<ChatHistoryEntry>,
    chat_history_scroll: usize,
    chat_history_viewport_height: usize,
    chat_history_total_lines: usize,
    pending_delivery_ids: HashSet<String>,
    terminal_delivery_message_ids: HashSet<String>,
    terminal_delivery_message_order: VecDeque<String>,
    seen_incoming_message_ids: HashSet<String>,
    seen_incoming_message_order: VecDeque<String>,
    seen_delivery_outcome_ids: HashSet<String>,
    seen_delivery_outcome_order: VecDeque<String>,
    relay_stream_poll_error_reported: bool,
    to_completion: Option<ToCompletionState>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(options: TuiLaunchOptions) -> Self {
        let TuiLaunchOptions {
            namespace,
            sender_session,
            relay_socket,
            look_lines,
            available_bundles,
            bindings,
        } = options;
        let relay_stream = RelayStreamSession::new(
            relay_socket.clone(),
            namespace.clone(),
            sender_session.clone(),
        );
        let mut state = Self {
            namespace,
            sender_session,
            relay_socket,
            relay_stream,
            look_lines,
            recipients: Vec::new(),
            cross_bundle_candidates: Vec::new(),
            bundle_status: None,
            last_selected_recipient: None,
            available_bundles,
            picker_open: false,
            picker_focus: PickerColumn::Sessions,
            picker_filter: String::new(),
            events_overlay_open: false,
            help_overlay_open: false,
            help_overlay_scroll: 0,
            help_overlay_page_rows: 0,
            help_overlay_maximum_scroll: 0,
            picker_session_state: ListState::default(),
            picker_bundle_state: ListState::default(),
            mode: ScreenMode::Communication,
            focus: FocusField::To,
            keyboard_enhancement: KeyboardEnhancement::default(),
            binding_configuration: bindings,
            // Replaced immediately below, once `self` exists to read the two
            // halves the table is built from.
            bindings: EffectiveBindings::default(),
            to_field: String::new(),
            to_cursor_index: 0,
            message_field: String::new(),
            message_cursor_index: 0,
            message_cursor_preferred_column: None,
            raww_draft: String::new(),
            raww_cursor_index: 0,
            raww_cursor_preferred_column: None,
            look_target: None,
            look_captured_at: None,
            look_snapshot_format: None,
            look_snapshot_lines: Vec::new(),
            look_snapshot_entries: Vec::new(),
            look_overlay_scroll: 0,
            look_choice_request_index: 0,
            look_choice_option_index: 0,
            // Deliberately empty. The startup line names a chord, and which
            // chord that is depends on the probe outcome, which has not
            // arrived yet. The render layer composes it against the effective
            // table as the footer's empty-history fallback.
            status_history: VecDeque::new(),
            event_history: VecDeque::new(),
            pending_choices: Vec::new(),
            pending_choices_state: ListState::default(),
            chat_history: VecDeque::new(),
            chat_history_scroll: 0,
            chat_history_viewport_height: 10,
            chat_history_total_lines: 0,
            pending_delivery_ids: HashSet::new(),
            terminal_delivery_message_ids: HashSet::new(),
            terminal_delivery_message_order: VecDeque::new(),
            seen_incoming_message_ids: HashSet::new(),
            seen_incoming_message_order: VecDeque::new(),
            seen_delivery_outcome_ids: HashSet::new(),
            seen_delivery_outcome_order: VecDeque::new(),
            relay_stream_poll_error_reported: false,
            to_completion: None,
            should_quit: false,
        };
        state.rebuild_bindings();
        state
    }

    /// Records the startup probe outcome and rebuilds the effective table for
    /// the capability class it puts this terminal in.
    pub(crate) fn set_keyboard_enhancement(&mut self, enhancement: KeyboardEnhancement) {
        self.keyboard_enhancement = enhancement;
        self.rebuild_bindings();
    }

    /// The startup probe outcome, as the help overlay reports it.
    pub(crate) fn keyboard_enhancement(&self) -> KeyboardEnhancement {
        self.keyboard_enhancement
    }

    /// Builds the bindings in force from the operator's configuration and the
    /// probe outcome now in hand.
    ///
    /// The platform is committed to here rather than passed in: this is the one
    /// caller that has to answer for the machine the TUI is actually running
    /// on, and every other caller of `EffectiveBindings::build` supplies it as
    /// an argument precisely so both arms stay reachable from a test.
    fn rebuild_bindings(&mut self) {
        self.bindings = EffectiveBindings::build(
            self.binding_configuration.as_ref(),
            &[],
            CapabilityClass::of(self.keyboard_enhancement.disambiguates_modified_keys()),
            cfg!(target_os = "macos"),
        );
    }

    /// Asks the event loop to shut down after the current iteration.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn push_status(&mut self, code: Option<String>, message: impl Into<String>) {
        self.status_history.push_front(StatusEntry {
            code,
            message: message.into(),
        });
        while self.status_history.len() > STATUS_HISTORY_MAXIMUM {
            self.status_history.pop_back();
        }
    }

    pub fn push_runtime_error(&mut self, error: RuntimeError) {
        match error {
            RuntimeError::Validation { code, message } => {
                self.push_status(Some(code), message);
            }
            RuntimeError::InvalidArgument { argument, message } => {
                self.push_status(
                    Some("validation_invalid_arguments".to_string()),
                    format!("invalid argument {argument}: {message}"),
                );
            }
            other => self.push_status(None, other.to_string()),
        }
    }
}

fn map_relay_error(error: RelayError) -> RuntimeError {
    // Preserve the canonical terminal relay codes the tui-surface raww error
    // taxonomy enumerates as stable, machine-readable status codes: the
    // `validation_*` family, `relay_unavailable`, and `authorization_forbidden`.
    // Without the explicit `authorization_forbidden` arm a relay-enforced
    // permission denial collapses into a generic IO status with no code, even
    // though the spec requires it to surface terminally with its code intact.
    // Every other (internal) code stays an IO status so unexpected relay
    // internals do not leak a code the surface would treat as actionable — but
    // the code is retained in the diagnostic message so the real cause is not
    // collapsed into one opaque string.
    if error.code.starts_with("validation_")
        || error.code == "relay_unavailable"
        || error.code == "authorization_forbidden"
    {
        return RuntimeError::validation(error.code, error.message);
    }
    RuntimeError::io(
        error.message,
        io::Error::other(format!("relay error {}", error.code)),
    )
}

fn map_relay_request_failure(socket_path: &std::path::Path, source: io::Error) -> RuntimeError {
    if is_relay_timeout_error(&source) {
        return RuntimeError::validation(
            "relay_timeout",
            format!(
                "relay timed out at {}; relay may be saturated or unresponsive",
                socket_path.display()
            ),
        );
    }
    if is_relay_unavailable_error(&source) {
        return RuntimeError::validation(
            "relay_unavailable",
            format!(
                "relay is unavailable at {}; start agentmux host relay with matching state-directory",
                socket_path.display()
            ),
        );
    }
    RuntimeError::io(
        format!("relay request failed for {}", socket_path.display()),
        source,
    )
}

fn is_relay_timeout_error(source: &io::Error) -> bool {
    matches!(source.kind(), io::ErrorKind::TimedOut)
}

fn is_relay_unavailable_error(source: &io::Error) -> bool {
    matches!(
        source.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}
