use std::path::Path;

use regex::Regex;

use crate::configuration::{
    ConfigurationError,
    fields::normalize_field,
    raw::{AcpTarget, PtyTarget, RawAcpTarget, RawPtyTarget, RawTmuxTarget, TmuxTarget},
    types::{AcpChannel, NameValueEntry, PromptReadinessTemplate},
};

pub(in crate::configuration) fn validate_tmux_target(
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

    Ok(TmuxTarget {
        initial_command: target.initial_command,
        resume_command: target.resume_command,
        prompt_regex: target.prompt_regex,
        prompt_inspect_lines: target.prompt_inspect_lines,
        prompt_idle_column: target.prompt_idle_column,
    })
}

pub(in crate::configuration) fn validate_acp_target(
    target: RawAcpTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<AcpTarget, ConfigurationError> {
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

    Ok(AcpTarget {
        channel: target.channel,
        command: target.command,
        url: target.url,
        headers: target.headers,
    })
}

/// Validates an ACP `name`/`value` header list: both fields non-empty after
/// trimming. Headers are an HTTP concept with looser key rules than an OS
/// environment variable, so this is intentionally distinct from
/// [`validate_environment_entries`].
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

/// Validates an `environment` array declared at the coder, bundle, or session
/// level. `context` is the caller's subject phrase (e.g. `coder 'acp'`,
/// `bundle 'alpha'`, `session 'a'`) so the structured error names the declaring
/// scope.
///
/// Unlike the looser header validator, an environment `name` must be a usable
/// OS environment-variable key: non-empty (after trimming) and free of `=` and
/// NUL. Both characters are unrepresentable as an env key —
/// `std::process::Command::env` panics on them, and tmux `-e KEY=VALUE` would
/// parse a `=`-bearing name ambiguously — so an invalid name must fail at
/// configuration load rather than at spawn. The `value` MAY be empty (a
/// legitimate OS environment value; the field's presence is already required by
/// the struct) but must be free of NUL.
pub(in crate::configuration) fn validate_environment_entries(
    entries: &[NameValueEntry],
    path: &Path,
    context: &str,
) -> Result<(), ConfigurationError> {
    for (index, entry) in entries.iter().enumerate() {
        let name = entry.name.as_str();
        if normalize_field(name).is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("{context} environment entry {index} has empty name"),
            ));
        }
        if name.contains('=') || name.contains('\0') {
            return Err(ConfigurationError::invalid(
                path,
                format!(
                    "{context} environment entry {index} name '{name}' must not contain '=' or NUL"
                ),
            ));
        }
        if entry.value.contains('\0') {
            return Err(ConfigurationError::invalid(
                path,
                format!("{context} environment entry {index} value must not contain NUL"),
            ));
        }
    }
    Ok(())
}

pub(super) fn prompt_readiness_from_tmux_target(
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

pub(super) fn prompt_readiness_from_pty_target(
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

pub(in crate::configuration) fn validate_pty_target(
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
        cols: target.cols,
        rows: target.rows,
        term_protocol: target.term_protocol,
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
