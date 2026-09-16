//! Validated delivery targets for bundle sessions.

pub(super) mod session;
pub(super) mod template;
pub(super) mod validation;

pub(super) use session::{build_session_target, select_marker_session_type};
pub(super) use template::ProjectNameInputs;
pub(super) use validation::{
    validate_acp_target, validate_environment_entries, validate_pty_target, validate_tmux_target,
};
