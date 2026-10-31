//! Adapter conformance suite: executable checks for the frozen agent-adapter
//! wire contract, driven through the harness-side supervisor double.
//!
//! Layout: `harness` plays supervisor over the reference adapter's stdio
//! pipe; round-trip equivalence drafts live in `round_trip_drafts.md` until
//! the canonical operations land; fixture-dependent checks (citation lint,
//! protocol-fixture presence) land with the first fixtures. Every case
//! asserts adapted-is-direct; no MCP request/response shape appears in any
//! case.

mod harness;

/// One wire-to-operation equivalence case: the wire payload traces to a
/// canonical operation input/output pair.
pub(crate) struct EquivalenceCase {
    pub(crate) name: &'static str,
    pub(crate) wire_method: &'static str,
    pub(crate) canonical_operation: &'static str,
}

/// The three `ideas/mcp/4` mappings. Draft status until the canonical
/// operations land; promotion turns each draft into an executable case.
pub(crate) const EQUIVALENCE_CASES: &[EquivalenceCase] = &[
    EquivalenceCase {
        name: "submit-content equivalence",
        wire_method: "adapter.submit",
        canonical_operation: "send",
    },
    EquivalenceCase {
        name: "choice round-trip equivalence",
        wire_method: "adapter.requestChoice",
        canonical_operation: "decide",
    },
    EquivalenceCase {
        name: "look-poll equivalence",
        wire_method: "relay.look",
        canonical_operation: "look",
    },
];

/// Methods that must never appear as an operation counterpart: `help`
/// (adapter-native, no operation) and supervision-only or liveness methods
/// (no operation counterpart). Protocol-level fixtures for these remain
/// required outside the equivalence set.
pub(crate) const EXCLUDED_FROM_EQUIVALENCE: &[(&str, &str)] = &[
    ("help", "adapter-native handler with no operation"),
    ("adapter.handshakeHello", "supervision-only establishment"),
    ("adapter.persistState", "supervision-only custody"),
    ("adapter.lifecycleEvent", "supervision-only signal"),
    ("relay.supervisionSignal", "supervision-only signal"),
    ("relay.ping", "liveness probe with no operation counterpart"),
];

#[test]
fn equivalence_cases_name_no_excluded_method() {
    for case in EQUIVALENCE_CASES {
        for (excluded, reason) in EXCLUDED_FROM_EQUIVALENCE {
            assert_ne!(
                case.wire_method, *excluded,
                "case '{}' maps excluded method {excluded}: {reason}",
                case.name,
            );
            assert_ne!(
                case.canonical_operation, *excluded,
                "case '{}' claims excluded operation {excluded}: {reason}",
                case.name,
            );
        }
    }
}

#[test]
fn equivalence_registry_covers_the_three_planned_mappings() {
    let operations: Vec<&str> = EQUIVALENCE_CASES
        .iter()
        .map(|case| case.canonical_operation)
        .collect();
    for expected in ["send", "decide", "look"] {
        assert!(
            operations.contains(&expected),
            "registry is missing the {expected} mapping"
        );
    }
}

#[test]
fn equivalence_case_names_are_unique() {
    let mut names: Vec<&str> =
        EQUIVALENCE_CASES.iter().map(|case| case.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), EQUIVALENCE_CASES.len());
}
