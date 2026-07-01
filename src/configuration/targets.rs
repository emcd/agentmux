use std::{collections::HashMap, path::Path};

use regex::Regex;

use super::{
    ConfigurationError,
    fields::{normalize_field, normalize_optional},
    raw::{
        AcpTarget, Coder, CoderTarget, PtyTarget, RawAcpTarget, RawPtyTarget, RawSession,
        RawTmuxTarget, TmuxTarget,
    },
    types::{
        AcpChannel, AcpTargetConfiguration, NameValueEntry, PromptReadinessTemplate,
        PtyTargetConfiguration, SessionType, TargetConfiguration, TmuxTargetConfiguration,
    },
};

/// Resolves a bundle member's validated delivery target.
///
/// A session is coder-backed when it carries a `coder` reference; its transport
/// (tmux or ACP) is derived from that coder's descriptor. Coder-less sessions
/// declare exactly one of the `[sessions.ui]` or `[sessions.pubsub]` markers.
pub(super) fn build_session_target(
    session: &RawSession,
    coders: &HashMap<String, Coder>,
    coders_path: &Path,
    bundle_path: &Path,
    session_id: &str,
) -> Result<(TargetConfiguration, Option<String>), ConfigurationError> {
    let kind = select_session_kind(
        session.coder.is_some(),
        session.ui.is_some(),
        session.pubsub.is_some(),
        bundle_path,
        session_id,
    )?;
    match kind {
        SessionKind::Coder => {
            let coder_session_id = normalize_optional(session.coder_session_id.as_deref());
            let coder = resolve_session_coder(
                session.coder.as_deref().unwrap_or_default(),
                coders,
                bundle_path,
                session_id,
            )?;
            match &coder.target {
                CoderTarget::Tmux(tmux_target) => {
                    let command_template = if coder_session_id.is_some() {
                        tmux_target.resume_command.as_str()
                    } else {
                        tmux_target.initial_command.as_str()
                    };
                    let start_command = render_command_template(
                        command_template,
                        coder_session_id.as_deref(),
                        bundle_path,
                        session_id,
                    )?;
                    let prompt_readiness =
                        prompt_readiness_from_tmux_target(tmux_target, coders_path, session_id)?;
                    Ok((
                        TargetConfiguration::Tmux(TmuxTargetConfiguration {
                            start_command,
                            prompt_readiness,
                            prime_timeout_ms: tmux_target.prime_timeout_ms,
                            wedge_detection: tmux_target.wedge_detection.unwrap_or(true),
                        }),
                        coder_session_id,
                    ))
                }
                CoderTarget::Acp(acp_target) => Ok((
                    TargetConfiguration::Acp(AcpTargetConfiguration {
                        channel: acp_target.channel,
                        command: acp_target.command.clone(),
                        url: acp_target.url.clone(),
                        prime_timeout_ms: acp_target.prime_timeout_ms,
                        headers: acp_target.headers.clone(),
                        environment: acp_target.environment.clone(),
                    }),
                    coder_session_id,
                )),
                CoderTarget::Pty(pty_target) => {
                    let command_template = if coder_session_id.is_some() {
                        pty_target.resume_command.as_str()
                    } else {
                        pty_target.initial_command.as_str()
                    };
                    let start_command = render_command_template(
                        command_template,
                        coder_session_id.as_deref(),
                        bundle_path,
                        session_id,
                    )?;
                    let prompt_readiness =
                        prompt_readiness_from_pty_target(pty_target, coders_path, session_id)?;
                    Ok((
                        TargetConfiguration::Pty(PtyTargetConfiguration {
                            initial_command: start_command.clone(),
                            resume_command: start_command,
                            prompt_readiness,
                            prime_timeout_ms: pty_target.prime_timeout_ms,
                            wedge_detection: pty_target.wedge_detection.unwrap_or(true),
                            cols: pty_target.cols.unwrap_or(120),
                            rows: pty_target.rows.unwrap_or(40),
                        }),
                        coder_session_id,
                    ))
                }
            }
        }
        SessionKind::Ui => {
            reject_coder_session_id(session, bundle_path, session_id)?;
            Ok((TargetConfiguration::Ui, None))
        }
        SessionKind::Pubsub => {
            reject_coder_session_id(session, bundle_path, session_id)?;
            Ok((TargetConfiguration::Pubsub, None))
        }
    }
}

/// Rejects a `coder-session-id` declared on a coder-less session entry.
fn reject_coder_session_id(
    session: &RawSession,
    bundle_path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if session.coder_session_id.is_some() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            format!("coder-less session '{session_id}' must not declare coder-session-id"),
        ));
    }
    Ok(())
}

fn resolve_session_coder<'a>(
    coder: &str,
    coders: &'a HashMap<String, Coder>,
    bundle_path: &Path,
    session_id: &str,
) -> Result<&'a Coder, ConfigurationError> {
    let coder_id = normalize_field(coder);
    if coder_id.is_empty() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            format!("session '{session_id}' coder reference must be non-empty"),
        ));
    }
    coders.get(coder_id).ok_or_else(|| {
        ConfigurationError::invalid(
            bundle_path,
            format!("session '{session_id}' references unknown coder '{coder_id}'"),
        )
    })
}

/// The declared shape of a bundle session: coder-backed, or one of the
/// coder-less marker types.
#[derive(Clone, Copy)]
enum SessionKind {
    Coder,
    Ui,
    Pubsub,
}

/// Selects the single declared session kind, rejecting a session that declares
/// neither a coder reference nor a coder-less marker, or more than one.
fn select_session_kind(
    has_coder: bool,
    has_ui: bool,
    has_pubsub: bool,
    path: &Path,
    session_id: &str,
) -> Result<SessionKind, ConfigurationError> {
    let declared = [
        (has_coder, SessionKind::Coder),
        (has_ui, SessionKind::Ui),
        (has_pubsub, SessionKind::Pubsub),
    ];
    let present: Vec<SessionKind> = declared
        .into_iter()
        .filter_map(|(declared, kind)| declared.then_some(kind))
        .collect();
    match present.as_slice() {
        [kind] => Ok(*kind),
        [] => Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' must declare a coder reference or exactly one \
                 coder-less session-type subtable ([sessions.ui] or [sessions.pubsub])"
            ),
        )),
        _ => Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' declares multiple session types; expected a coder \
                 reference or exactly one of [sessions.ui] or [sessions.pubsub]"
            ),
        )),
    }
}

/// Selects the session type for a global user, which is always coder-less.
pub(super) fn select_marker_session_type(
    has_ui: bool,
    has_pubsub: bool,
    path: &Path,
    session_id: &str,
) -> Result<SessionType, ConfigurationError> {
    match (has_ui, has_pubsub) {
        (true, false) => Ok(SessionType::Ui),
        (false, true) => Ok(SessionType::Pubsub),
        (false, false) => Err(ConfigurationError::invalid(
            path,
            format!(
                "users session '{session_id}' must declare exactly one session-type \
                 subtable ([sessions.ui] or [sessions.pubsub])"
            ),
        )),
        (true, true) => Err(ConfigurationError::invalid(
            path,
            format!(
                "users session '{session_id}' declares multiple session-type subtables; \
                 expected exactly one"
            ),
        )),
    }
}

pub(super) fn validate_tmux_target(
    target: RawTmuxTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<TmuxTarget, ConfigurationError> {
    if normalize_field(target.initial_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux initial-command must be non-empty"),
        ));
    }
    if normalize_field(target.resume_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux resume-command must be non-empty"),
        ));
    }

    if let Some(prompt_regex) = target.prompt_regex.as_deref() {
        if normalize_field(prompt_regex).is_empty() {
            return Err(ConfigurationError::invalid(
                coders_path,
                format!("coder '{coder_id}' tmux prompt-regex must be non-empty when set"),
            ));
        }
        compile_prompt_regex(prompt_regex, coders_path, coder_id, "tmux prompt-regex")?;
    }

    if matches!(target.prompt_inspect_lines, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux prompt-inspect-lines must be greater than zero"),
        ));
    }

    if matches!(target.prime_timeout_ms, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux prime-timeout-ms must be greater than zero"),
        ));
    }

    Ok(TmuxTarget {
        initial_command: target.initial_command,
        resume_command: target.resume_command,
        prompt_regex: target.prompt_regex,
        prompt_inspect_lines: target.prompt_inspect_lines,
        prompt_idle_column: target.prompt_idle_column,
        prime_timeout_ms: target.prime_timeout_ms,
        wedge_detection: target.wedge_detection,
    })
}

pub(super) fn validate_acp_target(
    target: RawAcpTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<AcpTarget, ConfigurationError> {
    if matches!(target.prime_timeout_ms, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' ACP prime-timeout-ms must be greater than zero"),
        ));
    }

    match target.channel {
        AcpChannel::Stdio => {
            let Some(command) = target.command.as_deref() else {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target requires non-empty command"),
                ));
            };
            if normalize_field(command).is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target requires non-empty command"),
                ));
            }
            if target.url.is_some() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target must not set url"),
                ));
            }
            if !target.headers.is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target must not set headers"),
                ));
            }
        }
        AcpChannel::Http => {
            let Some(url) = target.url.as_deref() else {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target requires non-empty url"),
                ));
            };
            if normalize_field(url).is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target requires non-empty url"),
                ));
            }
            if target.command.is_some() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target must not set stdio-only fields"),
                ));
            }
            validate_name_value_entries(&target.headers, coders_path, coder_id, "headers")?;
        }
    }

    validate_name_value_entries(&target.environment, coders_path, coder_id, "environment")?;

    Ok(AcpTarget {
        channel: target.channel,
        command: target.command,
        url: target.url,
        prime_timeout_ms: target.prime_timeout_ms,
        headers: target.headers,
        environment: target.environment,
    })
}

fn validate_name_value_entries(
    entries: &[NameValueEntry],
    path: &Path,
    coder_id: &str,
    field_name: &str,
) -> Result<(), ConfigurationError> {
    for (index, entry) in entries.iter().enumerate() {
        if normalize_field(entry.name.as_str()).is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("coder '{coder_id}' {field_name} entry {index} has empty name"),
            ));
        }
        if normalize_field(entry.value.as_str()).is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("coder '{coder_id}' {field_name} entry {index} has empty value"),
            ));
        }
    }
    Ok(())
}

fn render_command_template(
    template: &str,
    coder_session_id: Option<&str>,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let mut rendered = template.to_string();

    if rendered.contains("{coder-session-id}") {
        let Some(coder_session_id) = coder_session_id else {
            return Err(ConfigurationError::invalid(
                path,
                format!("session '{session_id}' requires coder-session-id for template"),
            ));
        };
        rendered = rendered.replace("{coder-session-id}", coder_session_id);
    }

    let placeholder_regex = Regex::new(r"\{[a-z][a-z0-9-]*\}").map_err(|source| {
        ConfigurationError::invalid(
            path,
            format!("internal placeholder regex failure: {source}"),
        )
    })?;
    if let Some(found) = placeholder_regex.find(rendered.as_str()) {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template has unknown placeholder '{}'",
                found.as_str()
            ),
        ));
    }

    if normalize_field(rendered.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' resolved command is empty"),
        ));
    }
    Ok(rendered)
}

fn prompt_readiness_from_tmux_target(
    target: &TmuxTarget,
    path: &Path,
    session_id: &str,
) -> Result<Option<PromptReadinessTemplate>, ConfigurationError> {
    let Some(prompt_regex) = target.prompt_regex.as_deref() else {
        return Ok(None);
    };
    compile_prompt_regex(prompt_regex, path, session_id, "prompt-regex")?;
    Ok(Some(PromptReadinessTemplate {
        prompt_regex: prompt_regex.to_string(),
        inspect_lines: target.prompt_inspect_lines,
        input_idle_cursor_column: target.prompt_idle_column,
    }))
}

fn prompt_readiness_from_pty_target(
    target: &PtyTarget,
    path: &Path,
    session_id: &str,
) -> Result<Option<PromptReadinessTemplate>, ConfigurationError> {
    let Some(prompt_regex) = target.prompt_regex.as_deref() else {
        return Ok(None);
    };
    compile_prompt_regex(prompt_regex, path, session_id, "pty prompt-regex")?;
    Ok(Some(PromptReadinessTemplate {
        prompt_regex: prompt_regex.to_string(),
        inspect_lines: target.prompt_inspect_lines,
        input_idle_cursor_column: target.prompt_idle_column,
    }))
}

pub(super) fn validate_pty_target(
    target: RawPtyTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<PtyTarget, ConfigurationError> {
    if normalize_field(target.initial_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty initial-command must be non-empty"),
        ));
    }
    if normalize_field(target.resume_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty resume-command must be non-empty"),
        ));
    }
    if let Some(prompt_regex) = target.prompt_regex.as_deref() {
        if normalize_field(prompt_regex).is_empty() {
            return Err(ConfigurationError::invalid(
                coders_path,
                format!("coder '{coder_id}' pty prompt-regex must be non-empty when set"),
            ));
        }
        compile_prompt_regex(prompt_regex, coders_path, coder_id, "pty prompt-regex")?;
    }
    if matches!(target.prompt_inspect_lines, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty prompt-inspect-lines must be greater than zero"),
        ));
    }
    if matches!(target.prime_timeout_ms, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty prime-timeout-ms must be greater than zero"),
        ));
    }
    if matches!(target.cols, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty cols must be greater than zero"),
        ));
    }
    if matches!(target.rows, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' pty rows must be greater than zero"),
        ));
    }
    Ok(PtyTarget {
        initial_command: target.initial_command,
        resume_command: target.resume_command,
        prompt_regex: target.prompt_regex,
        prompt_inspect_lines: target.prompt_inspect_lines,
        prompt_idle_column: target.prompt_idle_column,
        prime_timeout_ms: target.prime_timeout_ms,
        wedge_detection: target.wedge_detection,
        cols: target.cols,
        rows: target.rows,
    })
}

fn compile_prompt_regex(
    pattern: &str,
    path: &Path,
    session_id: &str,
    field_name: &str,
) -> Result<(), ConfigurationError> {
    Regex::new(pattern).map(|_| ()).map_err(|source| {
        ConfigurationError::invalid(
            path,
            format!("invalid {field_name} for session/coder '{session_id}': {source}"),
        )
    })
}
