use std::{path::PathBuf, sync::mpsc};

use crate::{configuration::BundleConfiguration, envelope::PromptBatchSettings};

use super::{ChatResult, DeliveryPayloadMode, RelayError, delivery::QuiescenceOptions};

#[derive(Clone, Debug)]
pub(super) struct ChatRequestContext {
    pub(super) request_id: Option<String>,
    pub(super) sender_session: String,
    pub(super) message: String,
    pub(super) targets: Vec<String>,
    pub(super) broadcast: bool,
    pub(super) quiet_window_ms: Option<u64>,
    pub(super) quiescence_timeout_ms: Option<u64>,
    pub(super) acp_turn_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) struct LookRequestContext {
    pub(super) requester_session: String,
    pub(super) target_session: String,
    pub(super) lines: Option<usize>,
    pub(super) bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RawwRequestContext {
    pub(super) request_id: Option<String>,
    pub(super) sender_session: String,
    pub(super) target_session: String,
    pub(super) text: String,
    pub(super) no_enter: bool,
    pub(super) bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct PermissionDecisionRequestContext {
    pub(super) permission_request_id: String,
    pub(super) outcome: String,
    pub(super) option_id: Option<String>,
    pub(super) bundle_name: Option<String>,
    pub(super) ui_session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RequestPrincipal {
    pub(super) session_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct AsyncDeliveryTask {
    pub(super) bundle: BundleConfiguration,
    pub(super) sender: crate::configuration::BundleMember,
    pub(super) all_target_sessions: Vec<String>,
    pub(super) target_session: String,
    pub(super) target_is_ui: bool,
    pub(super) message: String,
    pub(super) message_id: String,
    pub(super) quiescence: QuiescenceOptions,
    pub(super) batch_settings: PromptBatchSettings,
    pub(super) runtime_directory: PathBuf,
    pub(super) completion_sender: Option<mpsc::Sender<Result<ChatResult, RelayError>>>,
    pub(super) payload_mode: DeliveryPayloadMode,
    pub(super) append_enter: bool,
    pub(super) permission_decider_sessions: Vec<String>,
    pub(super) permission_max_pending: usize,
}
