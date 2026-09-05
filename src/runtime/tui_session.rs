//! TUI session configuration discovery and precedence resolution.

use crate::configuration::{
    ConfigurationError, ConfigurationRoots, TuiConfiguration, load_policy_ids,
    load_tui_configuration, load_ui_configuration,
};

use super::error::RuntimeError;
use crate::tui::BindingConfiguration;

const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";

/// Resolved TUI session identity for CLI/TUI operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTuiSession {
    pub namespace: String,
    pub session_selector: String,
    pub session_id: String,
    pub session_name: Option<String>,
    pub policy: String,
}

/// Loads active TUI configuration.
///
/// Overrides arrive through the configuration layers like every other
/// configuration file, rather than through a bespoke lookup rooted at the
/// working tree. That lookup was gated on build profile, so this override was
/// inert in release builds while its sibling in the same directory was not.
///
/// # Errors
///
/// Returns `RuntimeError` when configuration files are malformed.
pub fn load_active_tui_configuration(
    configuration_roots: &ConfigurationRoots,
) -> Result<Option<TuiConfiguration>, RuntimeError> {
    load_tui_configuration(configuration_roots)
        .map_err(|source| map_configuration_error(source, "load TUI configuration"))
}

/// Resolves TUI bundle/session identity with deterministic precedence.
///
/// Resolution order:
/// 1. explicit `--bundle` and `--as-session`
/// 2. `default-bundle` from active `ui.toml`, `default-session` from active
///    `users.toml`
/// 3. fail-fast validation errors
///
/// # Errors
///
/// Returns validation errors for missing selectors, unknown sessions, and
/// unknown policy references.
pub fn resolve_tui_session_identity(
    configuration_roots: &ConfigurationRoots,
    explicit_bundle: Option<&str>,
    explicit_session: Option<&str>,
) -> Result<ResolvedTuiSession, RuntimeError> {
    let configuration = load_active_tui_configuration(configuration_roots)?;
    let ui_default_bundle = load_ui_default_bundle(configuration_roots)?;
    let bundle_name = resolve_bundle_name(ui_default_bundle.as_deref(), explicit_bundle)?;
    finish_tui_session(
        configuration_roots,
        configuration.as_ref(),
        bundle_name,
        explicit_session,
    )
}

/// Resolves the interactive TUI launch identity, treating the browsing bundle
/// as optional.
///
/// The one-shot CLI commands operate on a concrete bundle and use
/// [`resolve_tui_session_identity`], which requires one. The interactive TUI, by
/// contrast, launches without requiring a configured `default-bundle`: the
/// operator selects a bundle in the picker. The browsing bundle is taken from
/// `explicit_bundle`, then the `ui.toml` `default-bundle`, then
/// `fallback_bundle` (e.g. the first available bundle), and is left empty when
/// none resolve. The session/policy identity is still resolved strictly.
///
/// # Errors
///
/// Returns validation errors for missing/unknown sessions and unknown policy
/// references — never for an absent bundle.
pub fn resolve_tui_launch_identity(
    configuration_roots: &ConfigurationRoots,
    explicit_bundle: Option<&str>,
    explicit_session: Option<&str>,
    fallback_bundle: Option<&str>,
) -> Result<ResolvedTuiSession, RuntimeError> {
    let configuration = load_active_tui_configuration(configuration_roots)?;
    let ui_default_bundle = load_ui_default_bundle(configuration_roots)?;
    let bundle_name = resolve_browsing_bundle(
        ui_default_bundle.as_deref(),
        explicit_bundle,
        fallback_bundle,
    );
    finish_tui_session(
        configuration_roots,
        configuration.as_ref(),
        bundle_name,
        explicit_session,
    )
}

/// Resolves the session selector, sender, and policy shared by both the strict
/// and lenient bundle-resolution entry points, then assembles the result around
/// an already-resolved `bundle_name`.
fn finish_tui_session(
    configuration_roots: &ConfigurationRoots,
    configuration: Option<&TuiConfiguration>,
    bundle_name: String,
    explicit_session: Option<&str>,
) -> Result<ResolvedTuiSession, RuntimeError> {
    let selector = resolve_session_selector(configuration, explicit_session)?;
    let selected = resolve_selected_session(configuration, selector.as_str())?;
    validate_sender_shape(selected.id.as_str())?;
    validate_selected_policy(configuration_roots, selected.policy.as_str())?;
    Ok(ResolvedTuiSession {
        namespace: bundle_name,
        session_selector: selector,
        session_id: selected.id.clone(),
        session_name: selected.name.clone(),
        policy: selected.policy.clone(),
    })
}

/// Loads the `ui.toml` `default-bundle` from the configuration layers.
///
/// Resolved through the layer list like every other configuration file, so an
/// earlier layer can supply surface defaults. A missing file resolves to `None`.
///
/// # Errors
///
/// Returns `RuntimeError` when `ui.toml` exists but is malformed.
fn load_ui_default_bundle(
    configuration_roots: &ConfigurationRoots,
) -> Result<Option<String>, RuntimeError> {
    Ok(load_ui_configuration(configuration_roots)
        .map_err(|source| map_configuration_error(source, "load UI configuration"))?
        .and_then(|configuration| configuration.default_bundle))
}

/// Loads the operator's `ui.toml` binding group from the configuration layers.
///
/// A sibling of [`load_ui_default_bundle`] rather than part of it: both read
/// the same file, and both are here so that the mapping from a configuration
/// fault to an operator-facing error has one definition. A missing file, or one
/// declaring no bindings, resolves to `None` and leaves the compiled defaults
/// in force.
///
/// # Errors
///
/// Returns `RuntimeError` when `ui.toml` is malformed or its binding group does
/// not validate.
pub fn load_ui_binding_configuration(
    configuration_roots: &ConfigurationRoots,
) -> Result<Option<BindingConfiguration>, RuntimeError> {
    Ok(load_ui_configuration(configuration_roots)
        .map_err(|source| map_configuration_error(source, "load UI configuration"))?
        .and_then(|configuration| configuration.bindings))
}

fn resolve_bundle_name(
    default_bundle: Option<&str>,
    explicit_bundle: Option<&str>,
) -> Result<String, RuntimeError> {
    if let Some(bundle_name) = explicit_bundle.and_then(normalize) {
        return Ok(bundle_name.to_string());
    }
    if let Some(bundle_name) = default_bundle.and_then(normalize) {
        return Ok(bundle_name.to_string());
    }
    Err(RuntimeError::validation(
        "validation_unknown_bundle",
        "bundle is required via --bundle or ui.toml default-bundle".to_string(),
    ))
}

/// Resolves the optional browsing bundle for an interactive TUI launch.
///
/// Unlike [`resolve_bundle_name`], an absent bundle is not an error: the TUI
/// launches with an empty browsing context and the operator picks a bundle in
/// the picker. Precedence is `explicit_bundle`, then the `ui.toml`
/// `default-bundle`, then `fallback_bundle`, then empty.
fn resolve_browsing_bundle(
    default_bundle: Option<&str>,
    explicit_bundle: Option<&str>,
    fallback_bundle: Option<&str>,
) -> String {
    explicit_bundle
        .and_then(normalize)
        .or_else(|| default_bundle.and_then(normalize))
        .or_else(|| fallback_bundle.and_then(normalize))
        .map(str::to_string)
        .unwrap_or_default()
}

fn resolve_session_selector(
    configuration: Option<&TuiConfiguration>,
    explicit_session: Option<&str>,
) -> Result<String, RuntimeError> {
    if let Some(session) = explicit_session.and_then(normalize) {
        return Ok(session.to_string());
    }
    if let Some(session) = configuration
        .and_then(|configuration| configuration.default_session.as_deref())
        .and_then(normalize)
    {
        return Ok(session.to_string());
    }
    Err(RuntimeError::validation(
        "validation_unknown_session",
        "session is required via --as-session or users.toml default-session".to_string(),
    ))
}

fn resolve_selected_session<'a>(
    configuration: Option<&'a TuiConfiguration>,
    selector: &str,
) -> Result<&'a crate::configuration::TuiSession, RuntimeError> {
    let Some(configuration) = configuration else {
        return Err(RuntimeError::validation(
            "validation_unknown_session",
            format!("session '{selector}' is not configured in users.toml"),
        ));
    };
    configuration.session_by_id(selector).ok_or_else(|| {
        RuntimeError::validation(
            "validation_unknown_session",
            format!("session '{selector}' is not configured in users.toml"),
        )
    })
}

fn validate_selected_policy(
    configuration_roots: &ConfigurationRoots,
    policy_id: &str,
) -> Result<(), RuntimeError> {
    let policy_ids = load_policy_ids(configuration_roots)
        .map_err(|source| map_configuration_error(source, "load policy presets"))?;
    if policy_ids.contains(policy_id) {
        return Ok(());
    }
    Err(RuntimeError::validation(
        "validation_unknown_policy",
        format!(
            "session policy '{}' is not configured in policies.toml",
            policy_id
        ),
    ))
}

fn validate_sender_shape(session_id: &str) -> Result<(), RuntimeError> {
    // Global users carry a `@GLOBAL` suffix; the local prefix follows the
    // bundle session-id grammar.
    let local = session_id
        .strip_suffix(GLOBAL_SESSION_SUFFIX)
        .unwrap_or(session_id);
    let Some(first) = local.chars().next() else {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            "session id is empty".to_string(),
        ));
    };
    if !first.is_ascii_alphabetic() {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            format!("session id '{session_id}' must start with an ASCII alphabetic character"),
        ));
    }
    if !local
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            format!(
                "session id '{session_id}' may only contain ASCII alphanumeric characters, '-' or '_'"
            ),
        ));
    }
    Ok(())
}

fn map_configuration_error(source: ConfigurationError, context: &str) -> RuntimeError {
    match source {
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!("{context} {}: {}", path.display(), message),
        ),
        ConfigurationError::Io { context, source } => RuntimeError::io(context, source),
        unreadable @ ConfigurationError::UnreadableConfigurationLayer { .. } => {
            RuntimeError::validation(
                "validation_unreadable_configuration_layer",
                format!("{context}: {unreadable}"),
            )
        }
        other => RuntimeError::validation(
            "validation_invalid_arguments",
            format!("{context}: {other}"),
        ),
    }
}

fn normalize(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}
