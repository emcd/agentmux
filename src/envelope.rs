//! Pane envelope rendering, parsing, and token-budget batching helpers.

use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt::{Display, Formatter},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ENVELOPE_SCHEMA_VERSION: &str = "1";
pub const RESERVED_PATH_POINTER_CONTENT_TYPE: &str = "application/vnd.tmuxmux.path-pointer+json";
pub const DEFAULT_MAX_PROMPT_TOKENS: usize = 4096;

const REQUIRED_HEADER_ENVELOPE_VERSION: &str = "Envelope-Version";
const REQUIRED_HEADER_MESSAGE_ID: &str = "Message-Id";
const REQUIRED_HEADER_DATE: &str = "Date";
const REQUIRED_HEADER_FROM: &str = "From";
const REQUIRED_HEADER_TO: &str = "To";
const REQUIRED_HEADER_CONTENT_TYPE: &str = "Content-Type";
const OPTIONAL_HEADER_CC: &str = "Cc";
const OPTIONAL_HEADER_SUBJECT: &str = "Subject";

/// Canonical machine-readable manifest line that starts each envelope.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManifestPreamble {
    pub schema_version: String,
    pub message_id: String,
    pub bundle_name: String,
    pub sender_session: String,
    pub target_sessions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc_sessions: Option<Vec<String>>,
    pub created_at: String,
}

/// Human-visible identity token for RFC 822-style headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressIdentity {
    pub session_name: String,
    pub display_name: Option<String>,
}

/// Input shape for rendering one RFC 822/MIME pane envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeRenderInput {
    pub manifest: ManifestPreamble,
    pub from: AddressIdentity,
    pub to: Vec<AddressIdentity>,
    pub cc: Vec<AddressIdentity>,
    pub subject: Option<String>,
    pub body: String,
}

/// Parsed envelope with canonical preamble, validated headers, and body text.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedEnvelope {
    pub manifest: ManifestPreamble,
    pub envelope_version: String,
    pub message_id: String,
    pub date: String,
    pub from: AddressIdentity,
    pub to: Vec<AddressIdentity>,
    pub cc: Vec<AddressIdentity>,
    pub subject: Option<String>,
    pub boundary: String,
    pub text_body: String,
    pub reserved_path_pointer_parts: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MimePart {
    content_type: String,
    body: String,
}

/// Tokenizer profile used for prompt-size estimation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenizerProfile {
    Characters0Point3,
    WhitespaceRough,
}

impl Default for TokenizerProfile {
    fn default() -> Self {
        Self::Characters0Point3
    }
}

/// Prompt batching settings for envelope injection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptBatchSettings {
    pub max_prompt_tokens: usize,
    pub tokenizer_profile: TokenizerProfile,
}

impl Default for PromptBatchSettings {
    fn default() -> Self {
        Self {
            max_prompt_tokens: DEFAULT_MAX_PROMPT_TOKENS,
            tokenizer_profile: TokenizerProfile::default(),
        }
    }
}

/// Envelope parse failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeParseError {
    message: String,
}

impl EnvelopeParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for EnvelopeParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for EnvelopeParseError {}

/// Renders one envelope as compact-manifest + RFC 822 headers + MIME body.
pub fn render_envelope(input: &EnvelopeRenderInput) -> String {
    let mut lines = Vec::new();
    lines.push(serde_json::to_string(&input.manifest).unwrap_or_else(|_| "{}".to_string()));

    let boundary = deterministic_boundary(&input.manifest.message_id);
    lines.push(format!(
        "{REQUIRED_HEADER_ENVELOPE_VERSION}: {}",
        input.manifest.schema_version
    ));
    lines.push(format!(
        "{REQUIRED_HEADER_MESSAGE_ID}: {}",
        input.manifest.message_id
    ));
    lines.push(format!(
        "{REQUIRED_HEADER_DATE}: {}",
        input.manifest.created_at
    ));
    lines.push(format!(
        "{REQUIRED_HEADER_FROM}: {}",
        render_address(&input.from)
    ));
    lines.push(format!(
        "{REQUIRED_HEADER_TO}: {}",
        input
            .to
            .iter()
            .map(render_address)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if !input.cc.is_empty() {
        lines.push(format!(
            "{OPTIONAL_HEADER_CC}: {}",
            input
                .cc
                .iter()
                .map(render_address)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(subject) = input.subject.as_deref() {
        let subject = subject.trim();
        if !subject.is_empty() {
            lines.push(format!("{OPTIONAL_HEADER_SUBJECT}: {subject}"));
        }
    }
    lines.push(format!(
        "{REQUIRED_HEADER_CONTENT_TYPE}: multipart/mixed; boundary=\"{boundary}\""
    ));
    lines.push(String::new());
    lines.push(format!("--{boundary}"));
    lines.push("Content-Type: text/plain; charset=utf-8".to_string());
    lines.push("Content-Transfer-Encoding: 8bit".to_string());
    lines.push(String::new());
    lines.push(input.body.clone());
    lines.push(format!("--{boundary}--"));
    lines.push(String::new());
    lines.join("\n")
}

/// Parses and validates one injected envelope.
pub fn parse_envelope(text: &str) -> Result<ParsedEnvelope, EnvelopeParseError> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut index = first_non_empty_line(&lines)
        .ok_or_else(|| EnvelopeParseError::new("missing manifest preamble"))?;
    let manifest_line = lines[index].trim();
    let manifest = serde_json::from_str::<ManifestPreamble>(manifest_line)
        .map_err(|_| EnvelopeParseError::new("manifest preamble is not valid JSON"))?;
    validate_manifest(&manifest)?;
    index += 1;

    let (headers, body_start_index) = parse_header_block(&lines, index)?;
    validate_required_headers(&headers)?;

    let boundary = parse_boundary_from_content_type(
        headers
            .get(REQUIRED_HEADER_CONTENT_TYPE)
            .ok_or_else(|| EnvelopeParseError::new("missing Content-Type header"))?,
    )?;
    let (parts, had_closing_boundary) = parse_mime_parts(&lines, body_start_index, &boundary)?;
    if !had_closing_boundary {
        return Err(EnvelopeParseError::new(
            "missing MIME closing boundary terminator",
        ));
    }

    let text_parts = parts
        .iter()
        .filter(|part| mime_type_matches(&part.content_type, "text/plain"))
        .collect::<Vec<_>>();
    if text_parts.len() != 1 {
        return Err(EnvelopeParseError::new(
            "required text/plain body part is missing or duplicated",
        ));
    }

    let reserved_path_pointer_parts = parts
        .iter()
        .filter(|part| mime_type_matches(&part.content_type, RESERVED_PATH_POINTER_CONTENT_TYPE))
        .filter_map(|part| serde_json::from_str::<Value>(part.body.trim()).ok())
        .collect::<Vec<_>>();

    let from = parse_address(
        headers
            .get(REQUIRED_HEADER_FROM)
            .ok_or_else(|| EnvelopeParseError::new("missing From header"))?,
    )?;
    let to = parse_address_list(
        headers
            .get(REQUIRED_HEADER_TO)
            .ok_or_else(|| EnvelopeParseError::new("missing To header"))?,
    )?;
    if to.is_empty() {
        return Err(EnvelopeParseError::new(
            "To header must include at least one recipient",
        ));
    }

    let cc = headers
        .get(OPTIONAL_HEADER_CC)
        .map(|value| parse_address_list(value))
        .transpose()?
        .unwrap_or_default();

    Ok(ParsedEnvelope {
        manifest,
        envelope_version: headers
            .get(REQUIRED_HEADER_ENVELOPE_VERSION)
            .cloned()
            .unwrap_or_default(),
        message_id: headers
            .get(REQUIRED_HEADER_MESSAGE_ID)
            .cloned()
            .unwrap_or_default(),
        date: headers
            .get(REQUIRED_HEADER_DATE)
            .cloned()
            .unwrap_or_default(),
        from,
        to,
        cc,
        subject: headers.get(OPTIONAL_HEADER_SUBJECT).cloned(),
        boundary,
        text_body: text_parts
            .first()
            .map(|part| part.body.clone())
            .unwrap_or_default(),
        reserved_path_pointer_parts,
    })
}

/// Renders one address token as `Display Name <session:session_name>`.
pub fn render_address(address: &AddressIdentity) -> String {
    let display = address
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(address.session_name.as_str())
        .replace('\"', "'");
    format!("{display} <session:{}>", address.session_name)
}

/// Parses one address token in `Display Name <session:session_name>` syntax.
pub fn parse_address(raw: &str) -> Result<AddressIdentity, EnvelopeParseError> {
    let value = raw.trim();
    let start = value.rfind("<session:").ok_or_else(|| {
        EnvelopeParseError::new("address is missing <session:...> identity token")
    })?;
    if !value.ends_with('>') {
        return Err(EnvelopeParseError::new(
            "address identity token must end with '>'",
        ));
    }

    let display = value[..start].trim();
    let session = value[start + "<session:".len()..value.len() - 1].trim();
    if session.is_empty() {
        return Err(EnvelopeParseError::new(
            "address session identity must be non-empty",
        ));
    }

    let display_name = if display.is_empty() {
        None
    } else {
        Some(display.to_string())
    };

    Ok(AddressIdentity {
        session_name: session.to_string(),
        display_name,
    })
}

/// Splits addresses from comma-separated list.
pub fn parse_address_list(raw: &str) -> Result<Vec<AddressIdentity>, EnvelopeParseError> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_address)
        .collect::<Result<Vec<_>, _>>()
}

/// Estimates prompt tokens using selected tokenizer profile.
pub fn estimate_prompt_tokens(text: &str, profile: TokenizerProfile) -> usize {
    let chars = text.chars().count();
    let lines = text.lines().count();
    let estimate = match profile {
        TokenizerProfile::Characters0Point3 => (chars * 3).div_ceil(10) + (lines / 12),
        TokenizerProfile::WhitespaceRough => text.split_whitespace().count() + (lines / 10),
    };
    estimate.max(1)
}

/// Batches envelopes into prompts under token budget while preserving order.
pub fn batch_envelopes(envelopes: &[String], settings: PromptBatchSettings) -> Vec<String> {
    if envelopes.is_empty() {
        return Vec::new();
    }

    let budget = settings.max_prompt_tokens.max(1);
    let mut batches = Vec::new();
    let mut current = String::new();

    for envelope in envelopes {
        if current.is_empty() {
            current.push_str(envelope);
            continue;
        }

        let candidate = format!("{current}\n\n{envelope}");
        let estimated = estimate_prompt_tokens(&candidate, settings.tokenizer_profile);
        if estimated <= budget {
            current = candidate;
            continue;
        }

        batches.push(current);
        current = envelope.clone();
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

/// Parses profile values for `TMUXMUX_TOKENIZER_PROFILE`.
pub fn parse_tokenizer_profile(value: &str) -> Option<TokenizerProfile> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "characters_0_point_3" | "character_0_point_3" | "chars_0_point_3" => {
            Some(TokenizerProfile::Characters0Point3)
        }
        "whitespace" | "whitespace_rough" => Some(TokenizerProfile::WhitespaceRough),
        _ => None,
    }
}

// TODO: Replace or supplement heuristic token estimates with real tokenizer
// integrations for canonical OpenAI encodings such as cl100k_base and
// o200k_base when a maintained Rust tokenizer crate is selected.

fn deterministic_boundary(message_id: &str) -> String {
    let normalized = message_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    if normalized.is_empty() {
        return "tmuxmux-boundary".to_string();
    }
    format!("tmuxmux-{normalized}")
}

fn first_non_empty_line(lines: &[&str]) -> Option<usize> {
    lines.iter().position(|line| !line.trim().is_empty())
}

fn validate_manifest(manifest: &ManifestPreamble) -> Result<(), EnvelopeParseError> {
    if manifest.schema_version.trim().is_empty() {
        return Err(EnvelopeParseError::new(
            "manifest schema_version is required",
        ));
    }
    if manifest.message_id.trim().is_empty() {
        return Err(EnvelopeParseError::new("manifest message_id is required"));
    }
    if manifest.bundle_name.trim().is_empty() {
        return Err(EnvelopeParseError::new("manifest bundle_name is required"));
    }
    if manifest.sender_session.trim().is_empty() {
        return Err(EnvelopeParseError::new(
            "manifest sender_session is required",
        ));
    }
    if manifest.created_at.trim().is_empty() {
        return Err(EnvelopeParseError::new("manifest created_at is required"));
    }
    if manifest.target_sessions.is_empty() {
        return Err(EnvelopeParseError::new(
            "manifest target_sessions is required",
        ));
    }
    Ok(())
}

fn parse_header_block(
    lines: &[&str],
    mut index: usize,
) -> Result<(BTreeMap<String, String>, usize), EnvelopeParseError> {
    let mut headers = BTreeMap::<String, String>::new();
    let mut seen = HashSet::<String>::new();
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() {
            return Ok((headers, index));
        }

        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| EnvelopeParseError::new("malformed header line"))?;
        let key = name.trim().to_string();
        if key.is_empty() {
            return Err(EnvelopeParseError::new("header name must be non-empty"));
        }
        if !seen.insert(key.clone()) {
            return Err(EnvelopeParseError::new(format!(
                "duplicate header '{key}' is not allowed"
            )));
        }
        headers.insert(key, value.trim().to_string());
    }
    Err(EnvelopeParseError::new(
        "missing blank line after header block",
    ))
}

fn validate_required_headers(headers: &BTreeMap<String, String>) -> Result<(), EnvelopeParseError> {
    let required = [
        REQUIRED_HEADER_ENVELOPE_VERSION,
        REQUIRED_HEADER_MESSAGE_ID,
        REQUIRED_HEADER_DATE,
        REQUIRED_HEADER_FROM,
        REQUIRED_HEADER_TO,
        REQUIRED_HEADER_CONTENT_TYPE,
    ];
    for header in required {
        if !headers.contains_key(header) {
            return Err(EnvelopeParseError::new(format!(
                "missing required header '{header}'"
            )));
        }
    }
    Ok(())
}

fn parse_boundary_from_content_type(value: &str) -> Result<String, EnvelopeParseError> {
    let mut pieces = value.split(';');
    let media_type = pieces
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if media_type != "multipart/mixed" {
        return Err(EnvelopeParseError::new(
            "Content-Type must be multipart/mixed",
        ));
    }

    for piece in pieces {
        let (name, parameter_value) = match piece.split_once('=') {
            Some(value) => value,
            None => continue,
        };
        if !name.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        let boundary = parameter_value.trim().trim_matches('"');
        if boundary.is_empty() {
            return Err(EnvelopeParseError::new(
                "multipart boundary parameter must be non-empty",
            ));
        }
        return Ok(boundary.to_string());
    }

    Err(EnvelopeParseError::new(
        "Content-Type multipart boundary parameter is required",
    ))
}

fn parse_mime_parts(
    lines: &[&str],
    mut index: usize,
    boundary: &str,
) -> Result<(Vec<MimePart>, bool), EnvelopeParseError> {
    let boundary_marker = format!("--{boundary}");
    let closing_marker = format!("--{boundary}--");
    let mut parts = Vec::<MimePart>::new();

    while index < lines.len() && lines[index].trim().is_empty() {
        index += 1;
    }
    if index >= lines.len() || lines[index].trim() != boundary_marker {
        return Err(EnvelopeParseError::new(
            "MIME body must start with boundary marker",
        ));
    }

    while index < lines.len() {
        let marker = lines[index].trim();
        if marker == closing_marker {
            return Ok((parts, true));
        }
        if marker != boundary_marker {
            return Err(EnvelopeParseError::new("invalid MIME boundary marker"));
        }
        index += 1;

        let (part_headers, next_index) = parse_header_block(lines, index)?;
        index = next_index;
        let content_type = part_headers
            .get("Content-Type")
            .cloned()
            .ok_or_else(|| EnvelopeParseError::new("MIME part is missing Content-Type"))?;

        let mut body_lines = Vec::<String>::new();
        while index < lines.len() {
            let line = lines[index];
            let trimmed = line.trim();
            if trimmed == boundary_marker || trimmed == closing_marker {
                break;
            }
            body_lines.push(line.to_string());
            index += 1;
        }
        if index >= lines.len() {
            return Err(EnvelopeParseError::new(
                "MIME body ended before closing boundary",
            ));
        }
        parts.push(MimePart {
            content_type,
            body: body_lines.join("\n"),
        });

        if lines[index].trim() == closing_marker {
            return Ok((parts, true));
        }
    }

    Ok((parts, false))
}

fn mime_type_matches(content_type: &str, expected: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    media_type == expected.to_ascii_lowercase()
}
