//! Fixture-script interpreter driving scripted turns.
//!
//! Turns follow a TOML fixture script: lifecycle events, scripted
//! `requestChoice` raises, acks per the trichotomy, snapshots for look
//! polls, and virtual-clock advances. Scripted `transport_ack` evidence
//! models a fake agent effect boundary and is not proof that any real
//! agent accepted input.

use serde::Deserialize;

use super::submit::OutcomeLiteral;

/// Scripted ack selection per turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptedAck {
    Submitted,
    RefusedBusy,
    TransportUnavailable,
    SubmissionUnknown,
}

impl ScriptedAck {
    /// Maps the scripted selection to the contract outcome literal.
    #[must_use]
    pub fn as_outcome(&self) -> OutcomeLiteral {
        match self {
            Self::Submitted => OutcomeLiteral::Submitted,
            Self::RefusedBusy => OutcomeLiteral::RefusedBusy,
            Self::TransportUnavailable => OutcomeLiteral::TransportUnavailable,
            Self::SubmissionUnknown => OutcomeLiteral::SubmissionUnknown,
        }
    }
}

/// One scripted choice raised during a turn.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedOption {
    pub option_id: String,
    pub name: String,
    pub species: String,
}

/// One scripted choice raised during a turn.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedChoice {
    pub title: String,
    pub species: String,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub deciders: Vec<String>,
    pub options: Vec<ScriptedOption>,
}

/// One scripted turn, keyed by the correlation the supervisor sends.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedTurn {
    pub correlation: String,
    pub ack: ScriptedAck,
    #[serde(default)]
    pub choices: Vec<ScriptedChoice>,
}

/// One scripted snapshot answering look polls for a target.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedSnapshot {
    pub target: String,
    pub freshness: String,
    #[serde(default)]
    pub entries: Vec<ScriptedEntry>,
}

/// One scripted snapshot entry; text is literal (base64 at render).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedEntry {
    pub kind: String,
    pub seq: u64,
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
}

/// One scripted lifecycle event to emit.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptedEvent {
    pub kind: String,
    pub code: String,
}

/// Virtual-clock schedule: advances applied to the injected clock in
/// trace order. Timeouts expire only on these events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ScriptedClock {
    #[serde(default)]
    pub advances_ms: Vec<u64>,
}

/// A parsed fixture script.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Script {
    #[serde(default)]
    pub turns: Vec<ScriptedTurn>,
    #[serde(default)]
    pub snapshots: Vec<ScriptedSnapshot>,
    #[serde(default)]
    pub events: Vec<ScriptedEvent>,
    #[serde(default)]
    pub clock: ScriptedClock,
}

/// Script parse or validation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    Malformed { reason: String },
    EmptyChoiceOptions { title: String },
}

impl std::fmt::Display for ScriptError {
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Self::Malformed { reason } => {
                write!(formatter, "malformed script: {reason}")
            },
            Self::EmptyChoiceOptions { title } => {
                write!(formatter, "scripted choice {title:?} has no options")
            },
        }
    }
}

impl std::error::Error for ScriptError {}

/// Parses and validates a TOML fixture script.
///
/// # Errors
///
/// Returns [`ScriptError::Malformed`] for unparseable TOML and
/// [`ScriptError::EmptyChoiceOptions`] for a choice without options
/// (empty options are a structural violation the wire refuses).
pub fn parse_script(text: &str) -> Result<Script, ScriptError> {
    let script: Script =
        toml::from_str(text).map_err(|source| ScriptError::Malformed {
            reason: source.to_string(),
        })?;
    for turn in &script.turns {
        for choice in &turn.choices {
            if choice.options.is_empty() {
                return Err(ScriptError::EmptyChoiceOptions {
                    title: choice.title.clone(),
                });
            }
        }
    }
    Ok(script)
}

impl Script {
    /// Finds the scripted turn for a submit correlation, if any.
    #[must_use]
    pub fn turn_for(&self, correlation_id: &str) -> Option<&ScriptedTurn> {
        self.turns
            .iter()
            .find(|turn| turn.correlation == correlation_id)
    }

    /// Finds the scripted snapshot for a look target, if any.
    #[must_use]
    pub fn snapshot_for(&self, target: &str) -> Option<&ScriptedSnapshot> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.target == target)
    }

    /// Returns the virtual-clock schedule in trace order.
    #[must_use]
    pub fn clock_advances(&self) -> &[u64] {
        &self.clock.advances_ms
    }
}
