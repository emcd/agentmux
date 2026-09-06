//! The chord shapes a binding row matches, and the two directions an
//! operator-facing chord travels: rendered for reading, parsed from writing.
//!
//! Every shape here matches exactly the keystrokes its written form denotes.
//! That is what lets dispatch, presentation and reachability agree without
//! coordinating: a row matching more than it spells is one a configuration
//! cannot fully claim, so its behavior survives being rebound and the overlay
//! and dispatch disagree about whether it is still there.

use crossterm::event::{KeyCode, KeyModifiers};

/// The pattern a row matches an incoming key against.
///
/// Three shapes, because there are three things a row can denote: one
/// keystroke, one character however a terminal reports its case, or typing.
/// Shapes that existed only to reproduce a handler condition -- one key under
/// any modifier, one character under any superset of `Ctrl` -- are gone rather
/// than narrowed, since a condition is not a chord and reproducing one is what
/// put keystrokes beyond an operator's reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Chord {
    /// One key with an exact modifier set, matching that keystroke and no
    /// other.
    Key(KeyCode, KeyModifiers),
    /// One character, bare or carrying `Shift`.
    ///
    /// The one shape denoting two keystrokes, and it denotes them because a
    /// terminal's report of a typed character is not a function of the key
    /// alone: `Shift` and `Caps Lock` each alter both which character arrives
    /// and which modifiers accompany it. Admitting only one of the two would
    /// refuse a keystroke an operator produced by typing. Between them, `c` and
    /// `C` so defined cover every way either letter can arrive.
    Char(char),
    /// Any character typed bare or with `Shift`. The character is carried into
    /// the action rather than into the row, since a row per character is not a
    /// thing a table can hold.
    Text,
}

impl Chord {
    /// How this chord is written for an operator. Presentation folds several
    /// rows onto one line, so a chord that renders the same as one already on
    /// the line disappears into it. Two rows reaching one action from
    /// `Ctrl+J` and from `Ctrl+Shift+J` would print both, since they are
    /// different keystrokes and render differently; what folds is a repeat of
    /// the same written form.
    pub(crate) fn display(self) -> String {
        match self {
            Self::Key(code, modifiers) => {
                format!(
                    "{}{}",
                    rendered_modifiers(modifiers),
                    rendered_key(code, modifiers)
                )
            }
            Self::Char(character) => character_display(character),
            Self::Text => "Type".to_string(),
        }
    }

    /// The keystroke a reader who copies this chord out of the overlay presses,
    /// or `None` where it stands for no one keystroke.
    ///
    /// The bare form for a character, since that is what its written form shows
    /// and what a reader will type; [`Chord::denoted_keystrokes`] answers the
    /// wider question of everything the row accepts.
    ///
    /// [`Chord::Text`] answers `None`. It stands for typing rather than for a
    /// key, which is the same reason it is outside the configuration grammar.
    pub(crate) fn denoted_keystroke(self) -> Option<(KeyCode, KeyModifiers)> {
        match self {
            Self::Key(code, modifiers) => Some((code, modifiers)),
            Self::Char(character) => Some((KeyCode::Char(character), KeyModifiers::NONE)),
            Self::Text => None,
        }
    }

    /// Every keystroke this chord's written form denotes.
    ///
    /// Exactly the set [`Chord::matches`] accepts, and the two are written to
    /// be read together: a shape whose match rule outgrew this list would be
    /// one a configuration could not fully claim, which is the condition
    /// exactness exists to remove.
    ///
    /// One entry for a key with a modifier set, two for a bare character, none
    /// for typing.
    pub(crate) fn denoted_keystrokes(self) -> Vec<(KeyCode, KeyModifiers)> {
        match self {
            Self::Key(code, modifiers) => vec![(code, modifiers)],
            Self::Char(character) => vec![
                (KeyCode::Char(character), KeyModifiers::NONE),
                (KeyCode::Char(character), KeyModifiers::SHIFT),
            ],
            Self::Text => Vec::new(),
        }
    }

    pub(super) fn matches(self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match self {
            Self::Key(row_code, row_modifiers) => code == row_code && modifiers == row_modifiers,
            Self::Char(character) => code == KeyCode::Char(character) && is_typed(modifiers),
            Self::Text => matches!(code, KeyCode::Char(_)) && is_typed(modifiers),
        }
    }
}

/// Whether a character reached the terminal as ordinary typing rather than as
/// part of a modified chord.
fn is_typed(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn key_code_display(code: KeyCode) -> String {
    match code {
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::F(number) => format!("F{number}"),
        KeyCode::Char(character) => character_display(character),
        other => format!("{other:?}"),
    }
}

fn character_display(character: char) -> String {
    match character {
        ' ' => "Space".to_string(),
        other => other.to_string(),
    }
}

/// The literal modifier the symbolic `primary` modifier resolves to on macOS.
///
/// The two coexist there carrying different meanings -- `Ctrl` is the terminal
/// and readline modifier, `Cmd` the application-command modifier -- so this
/// selects between them rather than treating either as an alias for the other.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrimaryModifier {
    /// The terminal and readline modifier, and the default.
    #[default]
    Control,
    /// The macOS application-command modifier, reported as `SUPER`.
    Command,
}

impl PrimaryModifier {
    /// The literal modifier this selection denotes.
    #[must_use]
    pub const fn modifier(self) -> KeyModifiers {
        match self {
            Self::Control => KeyModifiers::CONTROL,
            Self::Command => KeyModifiers::SUPER,
        }
    }
}

/// Resolves the symbolic `primary` modifier for a platform.
///
/// Off macOS the answer is always `Ctrl`: those platforms carry no second
/// application-command modifier for the symbol to choose between, so no
/// selection governs it. On macOS the operator's selection decides, defaulting
/// to `Ctrl` so a chord using the symbol is reachable everywhere without
/// depending on whether a given terminal delivers `Cmd` chords to the process
/// at all.
///
/// The platform arrives as an argument rather than through `cfg!` so both arms
/// are exercisable wherever the tests run.
#[must_use]
pub fn primary_modifier(on_macos: bool, selection: Option<PrimaryModifier>) -> KeyModifiers {
    if on_macos {
        selection.unwrap_or_default().modifier()
    } else {
        KeyModifiers::CONTROL
    }
}

/// Why an operator-written chord could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChordError {
    /// The text held no key, either because it was empty or because it ended on
    /// a modifier separator.
    Empty,
    /// A segment before the final one did not name a modifier.
    UnknownModifier(String),
    /// The final segment did not name a key.
    UnknownKey(String),
    /// A modifier appeared more than once.
    RepeatedModifier(String),
}

impl std::fmt::Display for ChordError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "chord names no key"),
            Self::UnknownModifier(segment) => {
                write!(formatter, "unknown modifier in chord: {segment}")
            }
            Self::UnknownKey(segment) => write!(formatter, "unknown key in chord: {segment}"),
            Self::RepeatedModifier(segment) => {
                write!(formatter, "modifier repeated in chord: {segment}")
            }
        }
    }
}

impl std::error::Error for ChordError {}

/// One key with an exact modifier set, as an operator wrote it, before the
/// symbolic modifier has been resolved for a platform.
///
/// Parsing and resolving are separate because the symbol's meaning is not a
/// property of the text: the same configuration is read on every platform and
/// resolves differently on each.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChordPattern {
    code: KeyCode,
    literal: KeyModifiers,
    primary: bool,
}

impl ChordPattern {
    /// Whether this chord was written with the symbolic `primary` modifier.
    #[must_use]
    pub const fn uses_primary_modifier(self) -> bool {
        self.primary
    }

    /// The key and modifier set this chord denotes once the symbolic modifier
    /// has been resolved to a literal one.
    ///
    /// A character carrying `Ctrl` folds to lower case here, because that is
    /// what a terminal reports for it: `Ctrl+C` arrives as a lowercase `c`.
    /// Folding at resolution rather than at parse time is what lets
    /// [`ChordPattern::render`] reproduce the conventional capitalized spelling
    /// while the resolved chord still matches the key that was pressed. It also
    /// waits until the symbolic modifier is known, so `primary+c` folds only
    /// where `primary` resolved to `Ctrl`.
    #[must_use]
    pub fn resolve(self, primary: KeyModifiers) -> (KeyCode, KeyModifiers) {
        let mut modifiers = self.literal;
        if self.primary {
            modifiers |= primary;
        }
        let code = match self.code {
            KeyCode::Char(character) if modifiers.contains(KeyModifiers::CONTROL) => {
                KeyCode::Char(character.to_ascii_lowercase())
            }
            other => other,
        };
        (code, modifiers)
    }

    /// The row shape this written chord denotes, once the symbolic modifier has
    /// been resolved.
    ///
    /// This is where the two sides are made to denote the same thing. An
    /// operator writing a bare single character gets [`Chord::Char`] — the same
    /// shape a compiled row naming that character carries, denoting the
    /// character bare and carrying `Shift`. Resolving to the bare keystroke
    /// alone would leave the configured row claiming one of the two while the
    /// compiled row kept answering for the other, which is the exact condition
    /// exactness exists to remove, reappearing in the one shape exempted from
    /// it.
    ///
    /// A character written with any modifier, symbolic included, is a chord
    /// rather than typing, so it denotes that one keystroke.
    #[must_use]
    pub(crate) fn resolve_to_chord(self, primary: KeyModifiers) -> Chord {
        let (code, modifiers) = self.resolve(primary);
        match code {
            KeyCode::Char(character) if modifiers.is_empty() => Chord::Char(character),
            _ => Chord::Key(code, modifiers),
        }
    }

    /// This chord written as an operator would write it.
    ///
    /// The inverse of `parse_chord` over the text it accepts: rendering a
    /// parsed chord reproduces the text that produced it, which is what lets a
    /// chord be copied out of generated documentation and pasted into a
    /// configuration.
    #[must_use]
    pub fn render(self) -> String {
        let symbolic = if self.primary { "primary+" } else { "" };
        format!(
            "{symbolic}{}{}",
            rendered_modifiers(self.literal),
            rendered_key(self.code, self.literal)
        )
    }
}

/// Reads an operator-written chord: modifier segments separated by `+`, then a
/// key.
///
/// Accepts the spellings [`Chord::display`] emits, so any chord the generated
/// binding documentation shows can be pasted into a configuration and denote
/// what was shown. Modifier names are matched without regard to case; a key
/// naming a single character is not, since `c` and `C` are distinct bindings.
///
/// # Errors
///
/// Returns [`ChordError`] naming the offending segment when a modifier or key
/// is unrecognized, when a modifier repeats, or when no key is present.
pub fn parse_chord(text: &str) -> Result<ChordPattern, ChordError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ChordError::Empty);
    }
    let segments: Vec<&str> = trimmed.split('+').collect();
    let (key, modifiers) = segments
        .split_last()
        .expect("split always yields at least one segment");
    if key.is_empty() {
        return Err(ChordError::Empty);
    }
    let mut literal = KeyModifiers::NONE;
    let mut primary = false;
    for segment in modifiers {
        match segment.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => set_once(&mut literal, KeyModifiers::CONTROL, segment)?,
            "alt" | "option" => set_once(&mut literal, KeyModifiers::ALT, segment)?,
            "shift" => set_once(&mut literal, KeyModifiers::SHIFT, segment)?,
            "cmd" | "command" | "super" => set_once(&mut literal, KeyModifiers::SUPER, segment)?,
            "primary" => {
                if primary {
                    return Err(ChordError::RepeatedModifier((*segment).to_string()));
                }
                primary = true;
            }
            _ => return Err(ChordError::UnknownModifier((*segment).to_string())),
        }
    }
    let (code, implied) = parse_key(key)?;
    let (code, literal) = canonical_key(code, literal | implied);
    Ok(ChordPattern {
        code,
        literal,
        primary,
    })
}

/// Folds a spelling onto the representation a terminal actually reports.
///
/// `Shift+Tab` is the case that matters: crossterm delivers it as
/// [`KeyCode::BackTab`] with no modifier, and the compiled rows match on that,
/// so reading the spelling as `Tab` carrying `Shift` would produce a chord no
/// keystroke ever satisfies. Canonicalizing here rather than at match time
/// keeps one representation flowing through resolution and dispatch alike.
fn canonical_key(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
        return (KeyCode::BackTab, modifiers.difference(KeyModifiers::SHIFT));
    }
    (code, modifiers)
}

/// Adds a modifier, refusing a second appearance so a repeated segment is
/// reported rather than absorbed.
fn set_once(
    modifiers: &mut KeyModifiers,
    addition: KeyModifiers,
    segment: &str,
) -> Result<(), ChordError> {
    if modifiers.contains(addition) {
        return Err(ChordError::RepeatedModifier(segment.to_string()));
    }
    *modifiers |= addition;
    Ok(())
}

/// Reads the key segment, with the modifiers its spelling implies.
///
/// `Shift+Tab` is the one spelling carrying a modifier in the key itself, which
/// is how [`Chord::display`] renders `BackTab`; reading it back as `Tab` with
/// `Shift` denotes the same keystroke in the shape the grammar expresses.
fn parse_key(segment: &str) -> Result<(KeyCode, KeyModifiers), ChordError> {
    let code = match segment.to_ascii_lowercase().as_str() {
        "enter" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        lowered => {
            if let Some(number) = lowered.strip_prefix('f').and_then(|rest| rest.parse().ok()) {
                KeyCode::F(number)
            } else {
                let mut characters = segment.chars();
                match (characters.next(), characters.next()) {
                    (Some(character), None) => KeyCode::Char(character),
                    _ => return Err(ChordError::UnknownKey(segment.to_string())),
                }
            }
        }
    };
    Ok((code, KeyModifiers::NONE))
}

/// Renders the modifier half of a chord, in one canonical order.
///
/// The single place modifiers are spelled, read by both [`Chord::display`] and
/// [`ChordPattern::render`] so the two cannot disagree about a chord's written
/// form.
///
/// Every spelling emitted here is one `parse_chord` accepts. Whether a whole
/// chord reads back depends on its key as well: the shapes outside
/// [`Chord::Key`] are deliberately not expressible in the grammar.
fn rendered_modifiers(modifiers: KeyModifiers) -> String {
    let mut text = String::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        text.push_str("Ctrl+");
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        text.push_str("Cmd+");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        text.push_str("Alt+");
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        text.push_str("Shift+");
    }
    text
}

/// Renders the key half of a chord, undoing the case folding a control chord
/// carries.
///
/// A terminal reports `Ctrl+C` as a lowercase `c`, and the table stores it that
/// way, while the conventional spelling capitalizes it. Rendering follows the
/// convention so that what is shown and what parses are the same text.
fn rendered_key(code: KeyCode, modifiers: KeyModifiers) -> String {
    match code {
        KeyCode::Char(character) if modifiers.contains(KeyModifiers::CONTROL) => {
            character_display(character.to_ascii_uppercase())
        }
        other => key_code_display(other),
    }
}
