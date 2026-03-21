use crate::runtime::error::RuntimeError;

use super::{LifecycleAction, lifecycle};

pub(super) fn run_agentmux_down(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        lifecycle::print_down_help();
        return Ok(());
    }
    lifecycle::run_bundle_lifecycle(LifecycleAction::Down, arguments)
}
