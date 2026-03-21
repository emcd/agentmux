use crate::runtime::error::RuntimeError;

use super::{LifecycleAction, lifecycle};

pub(super) fn run_agentmux_up(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        lifecycle::print_up_help();
        return Ok(());
    }
    lifecycle::run_bundle_lifecycle(LifecycleAction::Up, arguments)
}
