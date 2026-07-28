use crate::runtime::error::RuntimeError;

use super::{arguments, help, mcp, relay};

pub(in crate::commands) async fn run_agentmux_host(
    arguments: &[String],
) -> Result<(), RuntimeError> {
    if arguments.is_empty() {
        return Err(RuntimeError::InvalidArgument {
            argument: "host".to_string(),
            message: "missing mode; expected relay or mcp".to_string(),
        });
    }
    if arguments[0] == "--help" || arguments[0] == "-h" {
        help::print_host_help();
        return Ok(());
    }

    match arguments[0].as_str() {
        "relay" => {
            if arguments[1..]
                .iter()
                .any(|value| value == "--help" || value == "-h")
            {
                help::print_host_relay_help();
                return Ok(());
            }
            relay::run_relay_host(arguments::parse_host_relay_arguments(&arguments[1..])?).await
        }
        "mcp" => {
            if arguments[1..]
                .iter()
                .any(|value| value == "--help" || value == "-h")
            {
                help::print_host_mcp_help();
                return Ok(());
            }
            // `host mcp` is identifiable here, so an argument fault is retained
            // and reported at tool time rather than failing startup. `--help`
            // above still exits: it is an operator at a shell, not a client
            // negotiating a tool surface. No partially parsed value survives a
            // failed parse -- the whole result is discarded.
            mcp::run_mcp_host(arguments::parse_host_mcp_arguments(&arguments[1..])).await
        }
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown host mode; expected relay or mcp".to_string(),
        }),
    }
}
