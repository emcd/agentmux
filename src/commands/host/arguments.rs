use crate::{
    commands::{McpHostArguments, RelayHostArguments, RuntimeArguments, shared},
    runtime::error::RuntimeError,
};

pub(super) fn parse_host_relay_arguments(
    arguments: &[String],
) -> Result<RelayHostArguments, RuntimeError> {
    let mut parsed = RelayHostArguments {
        no_autostart: false,
        require_session_credentials: None,
        watch_bundles: None,
        runtime: RuntimeArguments::default(),
    };
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--no-autostart" => parsed.no_autostart = true,
            "--require-credentials" => parsed.require_session_credentials = Some(true),
            "--no-watch" => parsed.watch_bundles = Some(false),
            value if !value.starts_with('-') => {
                return Err(RuntimeError::validation(
                    "validation_invalid_arguments",
                    format!("'{}' is not supported by relay host", value),
                ));
            }
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    Ok(parsed)
}

pub(super) fn parse_host_mcp_arguments(
    arguments: &[String],
) -> Result<McpHostArguments, RuntimeError> {
    let mut parsed = McpHostArguments::default();
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--default-bundle" => {
                parsed.default_bundle = Some(shared::take_value(
                    arguments,
                    &mut index,
                    "--default-bundle",
                )?);
            }
            "--session-name" => {
                parsed.session_name =
                    Some(shared::take_value(arguments, &mut index, "--session-name")?);
            }
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    Ok(parsed)
}
