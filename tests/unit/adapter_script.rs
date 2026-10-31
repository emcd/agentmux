use agentmux::adapter::script::parse_script;
use agentmux::adapter::submit::OutcomeLiteral;

/// A representative script parses: turns, snapshots, events, clock.
#[test]
fn representative_script_parses() {
    let script = parse_script(EXAMPLE).expect("example parses");
    let turn = script.turn_for("c-1").expect("turn present");
    assert_eq!(turn.ack.as_outcome(), OutcomeLiteral::Submitted);
    assert_eq!(turn.choices.len(), 1);
    assert_eq!(turn.choices[0].options.len(), 2);
    let snapshot = script.snapshot_for("seat-1").expect("snapshot present");
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(script.events.len(), 1);
    assert_eq!(script.clock_advances(), &[100, 250]);
}

/// Unknown correlations and targets miss without failing the parse.
#[test]
fn unknown_correlation_misses() {
    let script = parse_script(EXAMPLE).expect("example parses");
    assert!(script.turn_for("c-9").is_none());
    assert!(script.snapshot_for("elsewhere").is_none());
}

/// An empty options list is a structural violation at parse time.
#[test]
fn empty_choice_options_refuse() {
    let text = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"

[[turns.choices]]
title = "Deploy?"
species = "approval"
options = []
"#;
    let error = parse_script(text).expect_err("empty options must refuse");
    assert!(error.to_string().contains("Deploy?"));
}

/// An unknown ack selection fails the parse with the serde reason.
#[test]
fn unknown_ack_selection_fails() {
    let text = r#"
[[turns]]
correlation = "c-1"
ack = "eventually"
"#;
    let error = parse_script(text).expect_err("unknown ack must fail");
    assert!(error.to_string().contains("malformed script"));
}

/// Every ack spelling maps to its contract literal.
#[test]
fn ack_spellings_map_to_literals() {
    for (spelling, literal) in [
        ("submitted", OutcomeLiteral::Submitted),
        ("refused_busy", OutcomeLiteral::RefusedBusy),
        (
            "transport_unavailable",
            OutcomeLiteral::TransportUnavailable,
        ),
        ("submission_unknown", OutcomeLiteral::SubmissionUnknown),
    ] {
        let text =
            format!("[[turns]]\ncorrelation = \"c\"\nack = \"{spelling}\"\n");
        let script = parse_script(&text).expect("spelling parses");
        let turn = script.turn_for("c").expect("turn present");
        assert_eq!(turn.ack.as_outcome(), literal);
    }
}

/// The checked-in example fixture parses and matches the schema.
#[test]
fn example_fixture_parses() {
    let path = format!(
        "{}/tests/fixtures/adapter/example.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("example fixture exists");
    let script = parse_script(&text).expect("example parses");
    assert!(script.turn_for("c-1").is_some());
    assert!(script.snapshot_for("seat-1").is_some());
    assert_eq!(script.clock_advances(), &[100, 250]);
}

const EXAMPLE: &str = r#"
[clock]
advances_ms = [100, 250]

[[turns]]
correlation = "c-1"
ack = "submitted"

[[turns.choices]]
title = "Deploy?"
species = "approval"
details = "ship it"

[[turns.choices.options]]
option_id = "yes"
name = "Yes"
species = "confirm"

[[turns.choices.options]]
option_id = "no"
name = "No"
species = "confirm"

[[snapshots]]
target = "seat-1"
freshness = "live"

[[snapshots.entries]]
kind = "text"
seq = 1
text = "hello"

[[events]]
kind = "ready"
code = "adapter_ready"
"#;
