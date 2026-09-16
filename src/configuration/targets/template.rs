use std::path::Path;

use regex::Regex;

use crate::configuration::{
    ConfigurationError,
    fields::{normalize_field, validate_project_name},
};

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
    word_begin: usize,
    word_end: usize,
    entry_quote: QuoteState,
    entry_escaped: bool,
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
    uses_project_name: bool,
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

/// Feeds one template character at its byte offset, tracking the word-span
/// events the composition check needs: the byte where the current word
/// began (on an `AtBoundary` to `InWord` transition) and the span of each
/// word the lexer closes (on an `InWord` to `AtBoundary` transition).
fn feed_tracked(
    state: LexerState,
    byte: usize,
    character: char,
    word_start: &mut usize,
    words: &mut Vec<(usize, usize)>,
) -> LexerState {
    let next = feed_character(state.0, state.1, state.2, character);
    if state.1 == BoundaryState::AtBoundary && next.1 == BoundaryState::InWord {
        *word_start = byte;
    }
    if state.1 == BoundaryState::InWord && next.1 == BoundaryState::AtBoundary {
        words.push((*word_start, byte));
    }
    next
}

/// Feeds a template slice whose first byte sits at `base`, tracking word
/// spans across every character.
fn feed_tracked_text(
    state: LexerState,
    text: &str,
    base: usize,
    word_start: &mut usize,
    words: &mut Vec<(usize, usize)>,
) -> LexerState {
    let mut state = state;
    for (offset, character) in text.char_indices() {
        state = feed_tracked(state, base + offset, character, word_start, words);
    }
    state
}

/// The mutable walk state of one scanner pass: the lexer state, the
/// cursor into the template, the current word start, and the spans of
/// words the lexer has closed.
struct ScanWalk {
    state: LexerState,
    cursor: usize,
    word_start: usize,
    words: Vec<(usize, usize)>,
}

/// Feeds one template slice through the tracked lexer, returning the end
/// state and recording word-span events.
fn feed_walk(walk: &mut ScanWalk, template: &str, begin: usize, end: usize) -> LexerState {
    walk.state = feed_tracked_text(
        walk.state,
        &template[begin..end],
        begin,
        &mut walk.word_start,
        &mut walk.words,
    );
    walk.cursor = end;
    walk.state
}

/// Records one placeholder occurrence with its entry lexer state and the
/// byte where its containing word began.
fn record_occurrence(
    walk: &mut ScanWalk,
    occurrences: &mut Vec<PlaceholderContext>,
    begin: usize,
    end: usize,
) {
    let (entry_quote, entry_boundary, entry_escaped) = walk.state;
    let word_begin = if entry_boundary == BoundaryState::AtBoundary {
        begin
    } else {
        walk.word_start
    };
    occurrences.push(PlaceholderContext {
        begin,
        end,
        word_begin,
        word_end: usize::MAX,
        entry_quote,
        entry_escaped,
    });
}

/// Closes the trailing word when the template ends inside one, then
/// resolves every occurrence's word end from the closed word spans.
fn close_scan(walk: &mut ScanWalk, occurrences: &mut Vec<PlaceholderContext>, template_len: usize) {
    if walk.state.1 == BoundaryState::InWord {
        walk.words.push((walk.word_start, template_len));
    }
    for occurrence in &mut *occurrences {
        occurrence.word_end = walk
            .words
            .iter()
            .find(|(begin, end)| *begin <= occurrence.begin && occurrence.begin < *end)
            .map(|(_, end)| *end)
            .expect("placeholder occurrence sits inside a scanned word");
    }
}

fn scan_template(template: &str, pattern: &Regex) -> TemplateScan {
    let mut walk = ScanWalk {
        state: (QuoteState::Outside, BoundaryState::AtBoundary, false),
        cursor: 0,
        word_start: 0,
        words: Vec::new(),
    };
    let mut occurrences = Vec::new();
    for captures in pattern.captures_iter(template) {
        let occurrence = captures.get(0).expect("empty placeholder match");
        let cursor = walk.cursor;
        feed_walk(&mut walk, template, cursor, occurrence.start());
        let (begin, end) = (occurrence.start(), occurrence.end());
        record_occurrence(&mut walk, &mut occurrences, begin, end);
        feed_walk(&mut walk, template, begin, end);
    }
    let end = template.len();
    let cursor = walk.cursor;
    feed_walk(&mut walk, template, cursor, end);
    close_scan(&mut walk, &mut occurrences, end);
    TemplateScan { occurrences }
}

/// Reports whether a literal affix byte is shell-literal-safe inside an
/// unquoted composed word: ASCII letters and digits plus `-_.:/=,+@%`.
/// Every other byte — glob characters, expansions, operators, quotes,
/// escapes, whitespace (which can only sit in the span escape-continued),
/// comment/tilde/history markers, and caret — fails the word.
fn is_shell_literal_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b':' | b'/' | b'=' | b',' | b'+' | b'@' | b'%'
        )
}

/// Reports whether a word prefix ending in `=` has shell assignment shape
/// (`^[A-Za-z_][A-Za-z0-9_]*=`): a non-empty stem of name characters
/// closed by the equals sign.
fn is_assignment_shape(prefix_through_equals: &str) -> bool {
    let Some(stem) = prefix_through_equals.strip_suffix('=') else {
        return false;
    };
    let mut characters = stem.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => (),
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Names the adjacent placeholder kind of one occurrence overlapping a
/// composed word: the grammar-safe variables substitute shell-safe bytes,
/// while `{{coder-session-id}}` and unknown shapes fail the word.
fn check_adjacent_placeholder(
    text: &str,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    match occurrence_kind(text) {
        OccurrenceKind::BundleSessionId
        | OccurrenceKind::ProjectName
        | OccurrenceKind::SessionDirectory => Ok(()),
        OccurrenceKind::CoderSessionId => Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template must not compose \
                 {{{{session-directory}}}} with {{{{coder-session-id}}}} in one word: \
                 the coder id has no shell-safe grammar"
            ),
        )),
        OccurrenceKind::Unknown => Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' template has unknown placeholder '{text}'"),
        )),
    }
}

/// Rejects a directory token whose composed word could split, expand, or
/// resequence on the tmux shell handoff while tokenizing literally on
/// Pty: quoted or escaped placement, literal affixes outside the
/// shell-literal-safe set, adjacent variables without a shell-safe
/// grammar, or a word that could form a shell assignment. Placeholder
/// spans read as one ASCII letter for the assignment check, since
/// substituted values are dynamic.
fn check_directory_placement(
    template: &str,
    scan: &TemplateScan,
    index: usize,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    let occurrence = &scan.occurrences[index];
    check_directory_entry(occurrence, path, session_id)?;
    let word = &template[occurrence.word_begin..occurrence.word_end];
    let overlaps: Vec<&PlaceholderContext> = scan
        .occurrences
        .iter()
        .filter(|other| other.begin < occurrence.word_end && other.end > occurrence.word_begin)
        .collect();
    let lettered = letter_composed_word(
        template,
        occurrence.word_begin,
        occurrence.word_end,
        &overlaps,
        path,
        session_id,
    )?;
    check_assignment_shapes(&lettered, word, path, session_id)
}

/// Rejects an escaped or quoted directory token before the word grammar
/// runs: an escape-opened or quote-region placement would corrupt the
/// quoted rendering with literal quote characters or extra word bytes.
fn check_directory_entry(
    occurrence: &PlaceholderContext,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if occurrence.entry_escaped {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template must not escape \
                 {{{{session-directory}}}}"
            ),
        ));
    }
    if occurrence.entry_quote != QuoteState::Outside {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template must place \
                 {{{{session-directory}}}} in an unquoted word"
            ),
        ));
    }
    Ok(())
}

/// Rejects one literal affix byte outside the shell-literal-safe set,
/// appending safe bytes to the lettered word rendering.
fn push_literal_byte(
    lettered: &mut String,
    byte: u8,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if !is_shell_literal_safe(byte) {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template affixes \
                 {{{{session-directory}}}} with '{}', which is not shell-literal-safe",
                byte as char,
            ),
        ));
    }
    lettered.push(byte as char);
    Ok(())
}

/// Walks the literal bytes between the placeholder spans overlapping one
/// composed word, rejecting unsafe affixes and reading every placeholder
/// span as one ASCII letter. Returns the lettered rendering the
/// assignment-shape check reads.
fn letter_composed_word(
    template: &str,
    word_begin: usize,
    word_end: usize,
    overlaps: &[&PlaceholderContext],
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let mut lettered = String::with_capacity(word_end - word_begin);
    let mut cursor = word_begin;
    for overlap in overlaps {
        for byte in template[cursor..overlap.begin].bytes() {
            push_literal_byte(&mut lettered, byte, path, session_id)?;
        }
        check_adjacent_placeholder(&template[overlap.begin..overlap.end], path, session_id)?;
        lettered.push('A');
        cursor = overlap.end;
    }
    for byte in template[cursor..word_end].bytes() {
        push_literal_byte(&mut lettered, byte, path, session_id)?;
    }
    Ok(lettered)
}

/// Rejects a lettered composed word holding `=` in shell assignment
/// shape at any equals position.
fn check_assignment_shapes(
    lettered: &str,
    word: &str,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    for (position, byte) in lettered.bytes().enumerate() {
        if byte == b'=' && is_assignment_shape(&lettered[..=position]) {
            return Err(ConfigurationError::invalid(
                path,
                format!(
                    "session '{session_id}' template word '{word}' could form a shell assignment"
                ),
            ));
        }
    }
    Ok(())
}

/// Rejects unknown placeholder names and misplaced directory tokens.
/// Quote, affix-grammar, adjacency, and assignment-shape rejection applies
/// only to `{{session-directory}}`: the id and project tokens substitute
/// grammar-safe raw values, so they stay valid in any template context.
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
        uses_project_name: false,
    };
    for (index, occurrence) in scan.occurrences.iter().enumerate() {
        let text = &template[occurrence.begin..occurrence.end];
        match occurrence_kind(text) {
            OccurrenceKind::CoderSessionId => usage.uses_coder_session_id = true,
            OccurrenceKind::BundleSessionId => usage.uses_bundle_session_id = true,
            OccurrenceKind::SessionDirectory => {
                check_directory_placement(template, scan, index, path, session_id)?;
                usage.uses_session_directory = true;
            }
            OccurrenceKind::ProjectName => usage.uses_project_name = true,
            OccurrenceKind::Unknown => {
                return Err(ConfigurationError::invalid(
                    path,
                    format!("session '{session_id}' template has unknown placeholder '{text}'"),
                ));
            }
        }
    }
    Ok(usage)
}

/// The known-variable kind of one placeholder occurrence.
enum OccurrenceKind {
    CoderSessionId,
    BundleSessionId,
    SessionDirectory,
    ProjectName,
    Unknown,
}

/// Splits one placeholder occurrence into its brace shape and kind.
fn occurrence_kind(text: &str) -> OccurrenceKind {
    let (is_double, name) = if text.starts_with("{{") {
        (true, &text[2..text.len() - 2])
    } else {
        (false, &text[1..text.len() - 1])
    };
    match (is_double, name) {
        (true, "coder-session-id") => OccurrenceKind::CoderSessionId,
        (true, "bundle-session-id") => OccurrenceKind::BundleSessionId,
        (true, "session-directory") => OccurrenceKind::SessionDirectory,
        (true, "project-name") => OccurrenceKind::ProjectName,
        _ => OccurrenceKind::Unknown,
    }
}

/// Normalized project-name inputs for one session: the eager explicit
/// value and rule names plus the bundle-level rule and canonical bundle
/// id the lazy derivation may need.
pub(in crate::configuration) struct ProjectNameInputs<'a> {
    pub(in crate::configuration) explicit: Option<&'a str>,
    pub(in crate::configuration) session_rule: Option<&'a str>,
    pub(in crate::configuration) bundle_rule: Option<&'a str>,
    pub(in crate::configuration) bundle_id: &'a str,
}

/// The lazy project-name derivation context for one render: the declared
/// directory supplies the basename fallback.
pub(in crate::configuration) struct ProjectNameSource<'a> {
    pub(in crate::configuration) explicit: Option<&'a str>,
    pub(in crate::configuration) session_rule: Option<&'a str>,
    pub(in crate::configuration) bundle_rule: Option<&'a str>,
    pub(in crate::configuration) bundle_id: &'a str,
    pub(in crate::configuration) directory: &'a Path,
}

/// Derives the final path segment of the declared directory string: the
/// basename fallback and the `session-directory-basename` rule yield.
fn directory_basename(
    directory: &Path,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let text = directory
        .to_str()
        .expect("session directory is valid Unicode: it deserializes from a TOML string");
    text.rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ConfigurationError::invalid(
                path,
                format!(
                    "session '{session_id}' directory has no basename for project-name derivation"
                ),
            )
        })
}

/// Resolves the project name for a template that uses `{{project-name}}`:
/// the explicit override wins; else the applicable rule (session-level
/// beats bundle-level); else the directory basename. Every derived value
/// satisfies the project-name grammar — a non-conforming value fails load
/// rather than sanitizing silently.
fn resolve_project_name(
    source: &ProjectNameSource,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    if let Some(explicit) = source.explicit {
        return Ok(explicit.to_string());
    }
    let resolved = match source.session_rule.or(source.bundle_rule) {
        Some("bundle-name") => source.bundle_id.to_string(),
        Some("session-directory-basename") | None => {
            directory_basename(source.directory, path, session_id)?
        }
        Some(rule) => {
            return Err(ConfigurationError::invalid(
                path,
                format!(
                    "session '{session_id}' project-name-from rule '{rule}' must be \
                     'session-directory-basename' or 'bundle-name'"
                ),
            ));
        }
    };
    validate_project_name(
        resolved.as_str(),
        path,
        &format!("session '{session_id}' derived project-name"),
    )?;
    Ok(resolved)
}

/// Substitutes the validated known variables in one pass over the
/// classified occurrence spans: literal ranges copy verbatim and each
/// original occurrence appends its value. Substituted value bytes are
/// never inspected, so placeholder-shaped bytes inside a directory or a
/// coder id stay literal while genuine template occurrences substitute.
fn apply_substitutions(
    template: &str,
    scan: &TemplateScan,
    coder_session_id: Option<&str>,
    session_id: &str,
    directory: &str,
    project_name: Option<&str>,
    path: &Path,
) -> Result<String, ConfigurationError> {
    let mut rendered = String::with_capacity(template.len() + directory.len());
    let mut cursor = 0;
    for occurrence in &scan.occurrences {
        rendered.push_str(&template[cursor..occurrence.begin]);
        let text = &template[occurrence.begin..occurrence.end];
        match occurrence_kind(text) {
            OccurrenceKind::CoderSessionId => rendered.push_str(
                coder_session_id.expect("classifier requires a coder-session-id value when used"),
            ),
            OccurrenceKind::BundleSessionId => rendered.push_str(session_id),
            OccurrenceKind::SessionDirectory => {
                rendered.push_str(&shell_quote_word(directory));
            }
            OccurrenceKind::ProjectName => rendered
                .push_str(project_name.expect("project name resolves when the template uses it")),
            OccurrenceKind::Unknown => {
                return Err(ConfigurationError::invalid(
                    path,
                    format!("session '{session_id}' template has unknown placeholder '{text}'"),
                ));
            }
        }
        cursor = occurrence.end;
    }
    rendered.push_str(&template[cursor..]);
    Ok(rendered)
}

/// Renders a coder command template for one session.
///
/// Every placeholder occurrence in the original template is classified
/// before anything is substituted, and substituted value bytes are never
/// rescanned — so a directory containing brace-shaped text cannot read as
/// a template placeholder. The accepted variables are
/// `{{coder-session-id}}` (the session's value, required when it occurs),
/// `{{bundle-session-id}}` (the normalized session id, substituted raw —
/// the id charset admits no shell metacharacters),
/// `{{session-directory}}` (the session directory, rendered as a single
/// shell-quoted word so it arrives as one argument on both the tmux shell
/// handoff and the pty `shell_words` handoff, and required to occupy an
/// unquoted word of shell-literal-safe affixes and grammar-safe adjacent
/// variables so operator text cannot corrupt it), and `{{project-name}}`
/// (the resolved project name, substituted raw under a grammar that keeps
/// it shell-safe everywhere — resolved and validated only when the chosen
/// template uses it). Every other occurrence, including every
/// single-brace form, fails load as an unknown placeholder.
pub(in crate::configuration) fn render_command_template(
    template: &str,
    coder_session_id: Option<&str>,
    session_directory: &Path,
    project: &ProjectNameSource,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let pattern = placeholder_pattern(path)?;
    let scan = scan_template(template, &pattern);
    let usage = classify_template_placeholders(template, &scan, path, session_id)?;
    require_coder_session_id(&usage, coder_session_id, path, session_id)?;
    let project_name = usage
        .uses_project_name
        .then(|| resolve_project_name(project, path, session_id))
        .transpose()?;
    let directory = session_directory
        .to_str()
        .expect("session directory is valid Unicode: it deserializes from a TOML string");
    let rendered = apply_substitutions(
        template,
        &scan,
        coder_session_id,
        session_id,
        directory,
        project_name.as_deref(),
        path,
    )?;
    ensure_command_nonempty(rendered.as_str(), path, session_id)?;
    Ok(rendered)
}

/// Rejects a template using `{{coder-session-id}}` when the session
/// carries no value: the resume form requires the handle it names.
fn require_coder_session_id(
    usage: &TemplateUsage,
    coder_session_id: Option<&str>,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if usage.uses_coder_session_id && coder_session_id.is_none() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' requires coder-session-id for template"),
        ));
    }
    Ok(())
}

/// Rejects a rendered command that trims to empty: a template of only
/// whitespace or quotes around substituted values names no program.
fn ensure_command_nonempty(
    rendered: &str,
    path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if normalize_field(rendered).is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' resolved command is empty"),
        ));
    }
    Ok(())
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
