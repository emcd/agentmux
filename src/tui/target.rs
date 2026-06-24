use std::collections::HashSet;

use crate::runtime::error::RuntimeError;

const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";

/// Returns the bundle a sender is bound to, or `None` for a relay-wide sender.
///
/// A sender whose session id carries the `@GLOBAL` suffix is a relay-wide
/// principal with no bound bundle; otherwise the sender is bound to
/// `active_bundle`. The result governs bare-target qualification in
/// [`merge_tui_targets`]: a bundle-bound sender qualifies a bare target with its
/// bound bundle, while a relay-wide sender (`None`) has no namespace to borrow
/// and so rejects bare targets — it must name each target's `@namespace`
/// explicitly, since the relay derives `Send` routing solely from those
/// suffixes.
pub fn sender_bound_bundle<'a>(sender_session: &str, active_bundle: &'a str) -> Option<&'a str> {
    if sender_session.ends_with(GLOBAL_SESSION_SUFFIX) {
        None
    } else {
        Some(active_bundle)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToCompletionState {
    pub token_start: usize,
    pub leading_ws: usize,
    pub candidates: Vec<String>,
    pub candidate_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct RecipientTokenContext {
    pub token_start: usize,
    pub leading_ws: usize,
    pub query: String,
    pub at_prefixed: bool,
}

/// Parses one recipient identifier for TUI target workflows.
///
/// The emitted identifier is always fully qualified as `<session>@<bundle>`:
/// the relay rejects bare targets, so the client fills in the namespace before
/// dispatch.
///
/// Accepted forms:
/// - bare: `<session-id>` — qualified with the sender's bound bundle and emitted
///   as `<session-id>@<bound-bundle>`. A bare target from a relay-wide sender is
///   rejected, since there is no bound bundle to qualify it with.
/// - canonical: `<session-id>@<bundle>` — emitted verbatim, routing the target to
///   the named bundle (or to the relay-wide `@GLOBAL` registry for user
///   sessions).
/// - autocomplete-trigger prefix: a single leading `@` is permitted on either
///   form and stripped before parsing.
///
/// `bound_bundle` is the bundle the sender is bound to, or `None` for a
/// relay-wide (unbound) sender. A relay-wide sender must name each target's
/// `@namespace` explicitly because the relay derives `Send` routing for it
/// entirely from those suffixes.
pub fn parse_tui_target_identifier(
    identifier: &str,
    bound_bundle: Option<&str>,
) -> Result<String, RuntimeError> {
    let trimmed = identifier.trim();
    let after_prefix = trimmed.strip_prefix('@').unwrap_or(trimmed);
    if after_prefix.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            "target identifier must be non-empty",
        ));
    }
    if after_prefix.contains('/') {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            format!("target identifier '{after_prefix}' is invalid; '/' is not allowed"),
        ));
    }
    let mut parts = after_prefix.splitn(2, '@');
    let session = parts.next().unwrap_or("");
    let bundle = parts.next();
    if session.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            format!(
                "session identifier must be non-empty in 'session@bundle' (got '{after_prefix}')"
            ),
        ));
    }
    match bundle {
        None => match bound_bundle {
            Some(namespace) => Ok(format!("{session}@{namespace}")),
            None => Err(RuntimeError::validation(
                "validation_unqualified_target",
                format!(
                    "target '{session}' must be fully-qualified as 'session@bundle'; \
                     a relay-wide sender has no bound bundle to infer the namespace from"
                ),
            )),
        },
        Some("") => Err(RuntimeError::validation(
            "validation_unknown_target",
            format!(
                "bundle qualifier must be non-empty in 'session@bundle' (got '{after_prefix}')"
            ),
        )),
        Some(namespace) if namespace.contains('@') => Err(RuntimeError::validation(
            "validation_unknown_target",
            format!("target identifier '{after_prefix}' may contain at most one '@' separator"),
        )),
        Some(namespace) => Ok(format!("{session}@{namespace}")),
    }
}

/// Merges the To recipient field into a deterministic target set.
///
/// `bound_bundle` is the sender's bound bundle, or `None` for a relay-wide
/// sender; it is forwarded to [`parse_tui_target_identifier`] to govern bare-
/// target qualification.
pub fn merge_tui_targets(
    to_field: &str,
    bound_bundle: Option<&str>,
) -> Result<Vec<String>, RuntimeError> {
    let mut targets = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();

    for token in to_field
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = parse_tui_target_identifier(token, bound_bundle)?;
        if seen.insert(normalized.clone()) {
            targets.push(normalized);
        }
    }

    if targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_empty_targets",
            "provide at least one recipient in To",
        ));
    }

    Ok(targets)
}

/// Completes the current recipient token from a list of candidate identities.
pub fn autocomplete_recipient_input(field_value: &str, candidates: &[String]) -> Option<String> {
    let context = current_recipient_token_context(field_value)?;
    let selected = matching_recipient_candidates(&context.query, candidates)
        .first()
        .cloned()?;

    let mut next = String::from(&field_value[..context.token_start]);
    let token_slice = &field_value[context.token_start..];
    next.push_str(&token_slice[..context.leading_ws]);
    next.push_str(selected.as_str());
    Some(next)
}

pub(super) fn matching_recipient_candidates(query: &str, candidates: &[String]) -> Vec<String> {
    let mut matched = candidates
        .iter()
        .filter(|candidate| query.is_empty() || candidate.starts_with(query))
        .cloned()
        .collect::<Vec<_>>();
    matched.sort_unstable();
    matched
}

pub(super) fn current_recipient_token_context(field_value: &str) -> Option<RecipientTokenContext> {
    let token_start = field_value.rfind(',').map(|index| index + 1).unwrap_or(0);
    let token_slice = &field_value[token_start..];
    let leading_ws = token_slice
        .char_indices()
        .find_map(|(index, character)| {
            if character.is_whitespace() {
                None
            } else {
                Some(index)
            }
        })
        .unwrap_or(token_slice.len());
    let token_text = token_slice[leading_ws..].trim().to_string();
    if token_text.is_empty() {
        return None;
    }

    let (at_prefixed, query) = if let Some(rest) = token_text.strip_prefix('@') {
        (true, rest.to_string())
    } else {
        (false, token_text.clone())
    };

    Some(RecipientTokenContext {
        token_start,
        leading_ws,
        query,
        at_prefixed,
    })
}

pub(crate) fn append_recipient_token(field_value: &str, recipient: &str) -> String {
    let mut tokens = field_value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.iter().any(|token| token == recipient) {
        return field_value.to_string();
    }
    tokens.push(recipient.to_string());
    tokens.join(", ")
}
