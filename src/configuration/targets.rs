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
/// is derived from that coder's descriptor. Coder-less sessions declare exactly
/// one of the `[sessions.ui]` or `[sessions.pubsub]` markers.
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
                        &session.directory,
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
                    let start_command = render_command_template(
                        command_template,
                        coder_session_id.as_deref(),
                        &session.directory,
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

    Ok(TmuxTarget {
        initial_command: target.initial_command,
        resume_command: target.resume_command,
        prompt_regex: target.prompt_regex,
        prompt_inspect_lines: target.prompt_inspect_lines,
        prompt_idle_column: target.prompt_idle_column,
    })
}

pub(super) fn validate_acp_target(
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
pub(super) fn validate_environment_entries(
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

/// Quote state of the template scanner at one byte offset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    Outside,
    InsideSingle,
    InsideDouble,
}

/// Word-boundary state of the template scanner at one byte offset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundaryState {
    AtBoundary,
    InWord,
}

/// Quote, boundary, and escape state carried across scanner steps.
type LexerState = (QuoteState, BoundaryState, bool);

/// Placement context of one placeholder occurrence in the original template.
struct PlaceholderContext {
    begin: usize,
    end: usize,
    entry_quote: QuoteState,
    entry_boundary: BoundaryState,
    entry_escaped: bool,
    right_boundary_valid: bool,
}

/// Placeholder occurrences found by one scanner walk, in template order.
struct TemplateScan {
    occurrences: Vec<PlaceholderContext>,
}

/// Known variables occurring in the template.
struct TemplateUsage {
    uses_coder_session_id: bool,
    uses_bundle_session_id: bool,
    uses_session_directory: bool,
}

/// Advances POSIX shell lexer state over one template character: quote
/// tracking with backslash escape state (literal inside single quotes),
/// and word-boundary tracking where only unquoted unescaped IFS
/// whitespace (space, tab, newline) re-establishes a boundary.
fn feed_character(
    quote: QuoteState,
    boundary: BoundaryState,
    escaped: bool,
    character: char,
) -> LexerState {
    if escaped {
        return (quote, BoundaryState::InWord, false);
    }
    match (quote, character) {
        (QuoteState::Outside, '\\') => (quote, boundary, true),
        (QuoteState::Outside, '\'') => (QuoteState::InsideSingle, BoundaryState::InWord, false),
        (QuoteState::Outside, '"') => (QuoteState::InsideDouble, BoundaryState::InWord, false),
        (QuoteState::Outside, ' ' | '\t' | '\n') => (quote, BoundaryState::AtBoundary, false),
        (QuoteState::Outside, _) => (quote, BoundaryState::InWord, false),
        (QuoteState::InsideSingle, '\'') => (QuoteState::Outside, BoundaryState::InWord, false),
        (QuoteState::InsideSingle, _) => (quote, BoundaryState::InWord, false),
        (QuoteState::InsideDouble, '\\') => (quote, boundary, true),
        (QuoteState::InsideDouble, '"') => (QuoteState::Outside, BoundaryState::InWord, false),
        (QuoteState::InsideDouble, _) => (quote, BoundaryState::InWord, false),
    }
}

/// Advances lexer state over a template slice, returning the end state.
fn feed_text(quote: QuoteState, boundary: BoundaryState, escaped: bool, text: &str) -> LexerState {
    let mut state = (quote, boundary, escaped);
    for character in text.chars() {
        state = feed_character(state.0, state.1, state.2, character);
    }
    state
}

/// Compiles the placeholder occurrence pattern: double-brace first, so
/// the alternation matches `{{name}}` whole rather than its inner
/// single-brace span.
fn placeholder_pattern(path: &Path) -> Result<Regex, ConfigurationError> {
    Regex::new(r"\{\{([a-z][a-z0-9_-]*)\}\}|\{([a-z][a-z0-9_-]*)\}").map_err(|source| {
        ConfigurationError::invalid(
            path,
            format!("internal placeholder regex failure: {source}"),
        )
    })
}

/// Reports whether the byte after a placeholder occurrence starts a new
/// shell word: end of template, or whitespace reached in Outside quote
/// state with no active escape.
fn right_boundary_valid(template: &str, end: usize, state: LexerState) -> bool {
    match template[end..].chars().next() {
        None => true,
        Some(next) => {
            state.0 == QuoteState::Outside && !state.2 && matches!(next, ' ' | '\t' | '\n')
        }
    }
}
fn scan_template(template: &str, pattern: &Regex) -> TemplateScan {
    let mut occurrences = Vec::new();
    let mut state: LexerState = (QuoteState::Outside, BoundaryState::AtBoundary, false);
    let mut cursor = 0;
    for captures in pattern.captures_iter(template) {
        let occurrence = captures.get(0).expect("empty placeholder match");
        state = feed_text(
            state.0,
            state.1,
            state.2,
            &template[cursor..occurrence.start()],
        );
        let (entry_quote, entry_boundary, entry_escaped) = state;
        state = feed_text(state.0, state.1, state.2, occurrence.as_str());
        occurrences.push(PlaceholderContext {
            begin: occurrence.start(),
            end: occurrence.end(),
            entry_quote,
            entry_boundary,
            entry_escaped,
            right_boundary_valid: right_boundary_valid(template, occurrence.end(), state),
        });
        cursor = occurrence.end();
    }
    TemplateScan { occurrences }
}

/// Rejects a directory token outside a standalone unquoted word: quoted,
/// affixed, escape-continued, or escape-opened placement would corrupt the
/// quoted rendering with literal quote characters or extra word bytes.
fn check_directory_placement(
    occurrence: &PlaceholderContext,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if occurrence.entry_escaped
        || occurrence.entry_quote != QuoteState::Outside
        || occurrence.entry_boundary != BoundaryState::AtBoundary
        || !occurrence.right_boundary_valid
    {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template must place \
                 {{{{session-directory}}}} as a standalone unquoted word"
            ),
        ));
    }
    Ok(())
}

/// Rejects unknown placeholder names and misplaced directory tokens.
/// Quote and word-boundary rejection applies only to
/// `{{session-directory}}`: the id tokens substitute charset-safe raw
/// values, so they stay valid in any template context.
fn classify_template_placeholders(
    template: &str,
    scan: &TemplateScan,
    path: &Path,
    session_id: &str,
) -> Result<TemplateUsage, ConfigurationError> {
    let mut usage = TemplateUsage {
        uses_coder_session_id: false,
        uses_bundle_session_id: false,
        uses_session_directory: false,
    };
    for occurrence in &scan.occurrences {
        let text = &template[occurrence.begin..occurrence.end];
        let (is_double, name) = if text.starts_with("{{") {
            (true, &text[2..text.len() - 2])
        } else {
            (false, &text[1..text.len() - 1])
        };
        match (is_double, name) {
            (false, "coder-session-id") => usage.uses_coder_session_id = true,
            (true, "bundle-session-id") => usage.uses_bundle_session_id = true,
            (true, "session-directory") => {
                check_directory_placement(occurrence, path, session_id)?;
                usage.uses_session_directory = true;
            }
            _ => {
                return Err(ConfigurationError::invalid(
                    path,
                    format!("session '{session_id}' template has unknown placeholder '{text}'"),
                ));
            }
        }
    }
    Ok(usage)
}

/// Substitutes the validated known variables: raw ids, and the directory
/// as one shell-quoted word. Never rescans substituted value bytes.
fn apply_substitutions(
    template: &str,
    usage: &TemplateUsage,
    coder_session_id: Option<&str>,
    session_id: &str,
    directory: &str,
) -> String {
    let mut rendered = template.to_string();
    if usage.uses_coder_session_id
        && let Some(value) = coder_session_id
    {
        rendered = rendered.replace("{coder-session-id}", value);
    }
    if usage.uses_bundle_session_id {
        rendered = rendered.replace("{{bundle-session-id}}", session_id);
    }
    if usage.uses_session_directory {
        rendered = rendered.replace("{{session-directory}}", &shell_quote_word(directory));
    }
    rendered
}

/// Renders a coder command template for one session.
///
/// Every placeholder occurrence in the original template is classified
/// before anything is substituted, and substituted value bytes are never
/// rescanned — so a directory containing brace-shaped text cannot read as
/// a template placeholder. The known variables are `{coder-session-id}`
/// (the session's value, required when it occurs),
/// `{{bundle-session-id}}` (the normalized session id, substituted raw —
/// the id charset admits no shell metacharacters), and
/// `{{session-directory}}` (the session directory, rendered as a single
/// shell-quoted word so it arrives as one argument on both the tmux shell
/// handoff and the pty `shell_words` handoff, and required to occupy a
/// standalone unquoted word so operator quotes cannot corrupt it).
fn render_command_template(
    template: &str,
    coder_session_id: Option<&str>,
    session_directory: &Path,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let pattern = placeholder_pattern(path)?;
    let scan = scan_template(template, &pattern);
    let usage = classify_template_placeholders(template, &scan, path, session_id)?;
    if usage.uses_coder_session_id && coder_session_id.is_none() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' requires coder-session-id for template"),
        ));
    }
    let directory = session_directory
        .to_str()
        .expect("session directory is valid Unicode: it deserializes from a TOML string");
    let rendered = apply_substitutions(template, &usage, coder_session_id, session_id, directory);
    if normalize_field(rendered.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' resolved command is empty"),
        ));
    }
    Ok(rendered)
}

/// Quotes a value as a single POSIX shell word: wraps it in single quotes,
/// encoding each embedded `'` as `'\''`. Both template consumers honor
/// single quotes (the tmux `new-session` shell handoff and the pty
/// `shell_words` tokenizer), so quoting is transparent for simple values
/// and correct for spaces, quotes, and backslashes.
fn shell_quote_word(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    let mut chunks = value.split('\'');
    quoted.push_str(chunks.next().unwrap_or_default());
    for chunk in chunks {
        quoted.push_str("'\\''");
        quoted.push_str(chunk);
    }
    quoted.push('\'');
    quoted
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
