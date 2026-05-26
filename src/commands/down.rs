use crate::runtime::error::RuntimeError;

use super::{BundleAction, bundle};

pub(super) fn run_agentmux_down(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        bundle::print_down_help();
        return Ok(());
    }
    bundle::run_bundle_command(BundleAction::Down, arguments)
}
