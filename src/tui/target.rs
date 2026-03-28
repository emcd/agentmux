use std::collections::HashSet;

use crate::runtime::error::RuntimeError;

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
/// Accepted forms:
/// - local: `<session-id>`
pub fn parse_tui_target_identifier(
    identifier: &str,
    _associated_bundle: &str,
) -> Result<String, RuntimeError> {
    let trimmed = identifier.trim().trim_start_matches('@');
    if trimmed.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            "target identifier must be non-empty",
        ));
    }
    if trimmed.contains('/') {
        return Err(RuntimeError::validation(
            "validation_unknown_target",
            format!("target identifier '{trimmed}' is invalid; use session id only"),
        ));
    }
    Ok(trimmed.to_string())
}

/// Merges the To recipient field into a deterministic target set.
pub fn merge_tui_targets(
    to_field: &str,
    associated_bundle: &str,
) -> Result<Vec<String>, RuntimeError> {
    let mut targets = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();

    for token in to_field
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = parse_tui_target_identifier(token, associated_bundle)?;
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
