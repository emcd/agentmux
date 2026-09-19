use std::{collections::HashMap, path::Path};

use crate::configuration::{
    ConfigurationError,
    fields::{normalize_field, normalize_optional},
    raw::{Coder, CoderTarget, RawSession},
    types::{
        AcpTargetConfiguration, PtyTargetConfiguration, SessionType, TargetConfiguration,
        TmuxTargetConfiguration,
    },
};

use super::{
    template::{ProjectNameInputs, ProjectNameSource, render_command_template},
    validation::{prompt_readiness_from_pty_target, prompt_readiness_from_tmux_target},
};

/// Renders the chosen coder command for one session, pairing the declared
/// directory with the normalized project-name inputs for lazy derivation.
fn render_session_command(
    command_template: &str,
    coder_session_id: Option<&str>,
    session: &RawSession,
    project: &ProjectNameInputs<'_>,
    bundle_path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let source = ProjectNameSource {
        explicit: project.explicit,
        session_rule: project.session_rule,
        bundle_rule: project.bundle_rule,
        bundle_id: project.bundle_id,
        directory: &session.directory,
    };
    render_command_template(
        command_template,
        coder_session_id,
        &source,
        bundle_path,
        session_id,
    )
}
/// Resolves a bundle member's validated delivery target.
///
/// A session is coder-backed when it carries a `coder` reference; its transport
/// is derived from that coder's descriptor. Coder-less sessions declare exactly
/// one of the `[sessions.ui]` or `[sessions.pubsub]` markers.
pub(in crate::configuration) fn build_session_target(
    session: &RawSession,
    coders: &HashMap<String, Coder>,
    coders_path: &Path,
    bundle_path: &Path,
    session_id: &str,
    project: &ProjectNameInputs<'_>,
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
                    let start_command = render_session_command(
                        command_template,
                        coder_session_id.as_deref(),
                        session,
                        project,
                        bundle_path,
                        session_id,
                    )?;
                    let prompt_readiness =
                        prompt_readiness_from_tmux_target(tmux_target, coders_path, session_id)?;
                    Ok((
                        TargetConfiguration::Tmux(TmuxTargetConfiguration {
                            start_command,
                            prompt_readiness,
                        }),
                        coder_session_id,
                    ))
                }
                CoderTarget::Acp(acp_target) => Ok((
                    TargetConfiguration::Acp(AcpTargetConfiguration {
                        channel: acp_target.channel,
                        command: acp_target.command.clone(),
                        url: acp_target.url.clone(),
                        headers: acp_target.headers.clone(),
                    }),
                    coder_session_id,
                )),
                CoderTarget::Pty(pty_target) => {
                    let command_template = if coder_session_id.is_some() {
                        pty_target.resume_command.as_str()
                    } else {
                        pty_target.initial_command.as_str()
                    };
                    let start_command = render_session_command(
                        command_template,
                        coder_session_id.as_deref(),
                        session,
                        project,
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
                            cols: pty_target.cols.unwrap_or(120),
                            rows: pty_target.rows.unwrap_or(40),
                            term_protocol: pty_target.term_protocol.unwrap_or_default(),
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
pub(in crate::configuration) fn select_marker_session_type(
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
