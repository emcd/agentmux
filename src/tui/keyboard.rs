//! Progressive keyboard-enhancement (Kitty keyboard protocol) capability
//! detection.
//!
//! Terminals that implement the protocol report modified keys as unambiguous
//! `CSI u` sequences, which is what separates `Shift+Enter` and `Ctrl+Enter`
//! from a bare `Enter`. Terminals that do not implement it collapse all three
//! onto the same byte, so the three forms arrive here indistinguishable.
//!
//! That is the whole of what this module knows: which chords a terminal can
//! deliver distinctly. What a delivered chord then does is the binding table's,
//! and naming one here would put a second copy of that answer in the module
//! least able to keep it current.
//!
//! The capability is probed once, immediately after terminal setup and before
//! the event loop starts reading keys: the probe writes a query to the terminal
//! and consumes the reply from the same input queue the event loop drains, so
//! the two cannot run concurrently.

use std::io;

use crossterm::{
    event::{KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags},
    execute, terminal,
};

/// Outcome of the startup keyboard-enhancement probe.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyboardEnhancement {
    /// The terminal advertised the protocol and the flags were pushed;
    /// modified keys arrive disambiguated.
    Active,
    /// The terminal answered the probe without advertising the protocol.
    #[default]
    Unsupported,
    /// The probe could not complete — no controlling terminal, an I/O failure,
    /// or no answer before crossterm's query timeout. Bindings treat this the
    /// same as `Unsupported`, but it is reported separately: an unanswered
    /// probe and a terminal that answered "no" are different operator problems.
    ProbeFailed,
}

impl KeyboardEnhancement {
    /// Whether modified keys arrive disambiguated from their unmodified forms.
    pub fn disambiguates_modified_keys(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Operator-facing description of the probe outcome and how modified `Enter`
/// reaches the TUI under it. One line per element.
///
/// Delivery only: what a key does under an outcome is not this module's to say.
/// The report once ended by naming the chord that inserts a newline regardless
/// of the outcome, which made this a second place a binding was written down
/// and would have gone false the moment the row moved. The help renderer
/// appends that line now, generated from the binding table.
///
/// The `ProbeFailed` wording claims nothing about the terminal. A failed probe
/// establishes only that the TUI could not determine or enable disambiguation;
/// the terminal may well support the protocol.
pub fn format_keyboard_enhancement_lines(enhancement: KeyboardEnhancement) -> Vec<String> {
    let outcome = match enhancement {
        KeyboardEnhancement::Active => [
            "Kitty keyboard protocol: active",
            "Enter with modifiers is reported distinctly",
        ],
        KeyboardEnhancement::Unsupported => [
            "Kitty keyboard protocol: unsupported",
            "Enter with modifiers arrives as bare Enter",
        ],
        KeyboardEnhancement::ProbeFailed => [
            "Kitty keyboard protocol: probe failed",
            "Keyboard capability is undetermined",
        ],
    };
    outcome.iter().map(|line| (*line).to_string()).collect()
}

/// Owns the pushed enhancement flags for the lifetime of a TUI run.
///
/// Dropping pops the flags, so the terminal is left with the key-reporting mode
/// it had before launch. The guard must outlive the event loop and be dropped
/// before the terminal itself is restored.
pub(crate) struct KeyboardEnhancementSession {
    enhancement: KeyboardEnhancement,
}

impl KeyboardEnhancementSession {
    /// Probes the terminal and pushes the disambiguation flag when supported.
    ///
    /// Only `DISAMBIGUATE_ESCAPE_CODES` is requested. The remaining flags
    /// change which events are delivered at all — `REPORT_EVENT_TYPES` adds key
    /// release and repeat events, `REPORT_ALL_KEYS_AS_ESCAPE_CODES` suppresses
    /// text delivery for keys the compose fields read as characters — and
    /// nothing in the input layer consumes them.
    pub fn activate() -> Self {
        let enhancement = match terminal::supports_keyboard_enhancement() {
            Ok(true) => {
                match execute!(
                    io::stdout(),
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                ) {
                    Ok(()) => KeyboardEnhancement::Active,
                    Err(_) => KeyboardEnhancement::ProbeFailed,
                }
            }
            Ok(false) => KeyboardEnhancement::Unsupported,
            Err(_) => KeyboardEnhancement::ProbeFailed,
        };
        Self { enhancement }
    }

    pub fn enhancement(&self) -> KeyboardEnhancement {
        self.enhancement
    }
}

impl Drop for KeyboardEnhancementSession {
    fn drop(&mut self) {
        if self.enhancement.disambiguates_modified_keys() {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
    }
}
