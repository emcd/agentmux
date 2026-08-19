//! The override layers above `relay.toml`: environment variables and the
//! precedence rule that ranks them against a CLI flag and the documented
//! default.
//!
//! Reading process state and validating a value are split so the precedence and
//! the parsing are both testable without mutating the environment.

use serde_json::json;

use crate::relay::{RelayError, relay_error};

pub(super) const DEFAULT_WATCH_BUNDLES: bool = true;
pub(super) const DEFAULT_REQUIRE_SESSION_CREDENTIALS: bool = false;
pub(super) const ENV_WATCH_BUNDLES: &str = "AGENTMUX_RELAY_WATCH_BUNDLES";
pub(super) const ENV_REQUIRE_SESSION_CREDENTIALS: &str =
    "AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS";

/// Resolves one boolean relay setting through the precedence ladder: a CLI
/// override wins, then an environment override, then the `relay.toml` value,
/// then the documented default. Pure so the precedence is unit-testable without
/// touching process state.
#[must_use]
pub fn resolve_relay_bool_setting(
    cli_override: Option<bool>,
    environment_override: Option<bool>,
    file_value: Option<bool>,
    default: bool,
) -> bool {
    cli_override
        .or(environment_override)
        .or(file_value)
        .unwrap_or(default)
}

/// Parses a canonical boolean override value (`true`/`false` only) for the named
/// environment variable, surfacing a structured error for anything else. Split
/// from process-env reading so the validation is unit-testable without mutating
/// process state.
pub fn parse_relay_bool_env_value(variable: &str, value: &str) -> Result<bool, RelayError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(relay_error(
            "validation_invalid_arguments",
            "relay environment override must be exactly 'true' or 'false'",
            Some(json!({
                "variable": variable,
                "value": value,
                "expected": ["true", "false"],
            })),
        )),
    }
}

/// Reads a canonical boolean environment override. Returns `Ok(None)` when the
/// variable is absent (or holds non-UTF-8, treated as absent), `Ok(Some(_))` for
/// exactly `true`/`false`, and a structured error for any other value.
pub(super) fn relay_bool_env_override(variable: &str) -> Result<Option<bool>, RelayError> {
    match std::env::var(variable).ok() {
        None => Ok(None),
        Some(value) => parse_relay_bool_env_value(variable, value.as_str()).map(Some),
    }
}
