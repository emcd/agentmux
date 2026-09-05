## Why

`look-and-stream-events` Relay Look Operation still describes the look request
as it was before the shared config-free routing stage landed. Four of its
statements are contradicted by the corpus, by the protocol contract, or by both
— and two of them are scenarios, which makes them test cases for the wrong
answer rather than merely misleading prose.

The `raww` requirement that states the shared stage carries the opposite
problem: it narrates the change that introduced the rule ("After this change
...", "are removed in this change") and instructs the deletion of two Rust
items, neither of which any future state of the system can satisfy or violate.

No behavior is in question. Every check found the code and the majority of the
corpus in agreement, with the outlier text as the defect.

## What Changes

Relay Look Operation:

- **BREAKING (documentation only)** the bare-target allowance is withdrawn.
  The field list says `target_session` MAY be a bare session id resolved within
  the requester's bound bundle; `addressing-routing` Suffix-Based Target Routing
  forbids exactly that, and `src/relay/routing.rs` rejects it with
  `validation_unqualified_target`.
- The `bundle_name` request field is removed. `RelayRequest::Look` carries
  `requester_session`, `target_session`, `lines` and `offset`, and nothing else.
- The two scenarios asserting those behaviors are replaced by scenarios
  asserting the rejection that actually occurs.
- The relay-wide (`@GLOBAL`) arm is stated. It is currently absent, which leaves
  the neighbouring "Reject unknown peer bundle" scenario reading as though it
  covered a relay-wide target; the handler in fact takes a separate branch and
  answers `validation_unknown_target` or `validation_unsupported_operation`.
- The peer-bundle scenario stops asserting a `bundle_name` response field that
  Look Response Contract, three requirements below it in the same file, says is
  retired.
- The stated default look scope changes from `self` to `home`. The default was
  widened in both the code and the shipped `policies.toml` by the change that
  set the default look scope to all:home; the specs were not updated with it.
  `PolicyControls::conservative_default` and the `default` preset in
  `data/configuration/policies.toml` both read `home` today, and nothing in the
  corpus, the code, or the tests asserts `self`.

Relay raww target resolution and bundle boundary:

- The migration narration is replaced by the rule it was narrating, which the
  requirement already states for raww and now states once for both operations.
- The relay-wide rejection is split into its two actual arms. The requirement
  said an `@GLOBAL` target is rejected with `validation_unsupported_operation`,
  but `src/relay/handlers/raww.rs:302-315` reaches that only for a *registered*
  principal; an unregistered one is rejected with `validation_unknown_target`.
  This matches the arms now stated for look, which is what the requirement's own
  uniformity claim asserts.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `look-and-stream-events`: Relay Look Operation — request field list,
  relay-wide target handling, and four scenarios.
- `raww`: Relay raww target resolution and bundle boundary — migration voice
  replaced by rule voice; the rule itself is unchanged.

## Impact

Documentation-only. No code path changes, and no scenario that survives this
change asserts anything the current implementation does not already do.

Evidence: `src/relay/routing.rs:225-272` (`resolve_target`),
`src/relay/handlers/look.rs:155-173` (the relay-wide branch),
`src/relay/contract.rs:400-410` (the `Look` request) and `:515-532` (the `Look`
response).

`redesign-mailbox-delivery-protocol` also carries a `raww` delta, against a
different requirement (Relay raww transport behavior). The two do not overlap
textually, but they sync into the same file.
