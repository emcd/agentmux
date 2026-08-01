## Why

The Tmux implementation has an OpenCode-specific compose-region predicate
that prevents delivery while text is present in the input box, even when the
configured prompt regex matches. The transport-contracts specification still
describes readiness as regex matching plus the optional cursor column, so the
normative contract must describe this existing safety gate before the fix can
land.

## What Changes

- Modify prompt-readiness template gating to define the OpenCode-specific
  compose-region predicate used after a successful prompt-regex match.
- Define the measured OpenCode frame suffix and the three-row, 99/100-space
  layout boundary that scope the predicate.
- Define the mismatch semantics: a matching OpenCode frame with compose text
  is not ready and is not injected; other coders and non-matching frames keep
  their existing readiness behavior.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `transport-contracts`: make the OpenCode compose-region predicate and its
  readiness/mismatch semantics normative.

## Impact

- Updates the transport-contracts OpenSpec delta and its implementation in
  `src/tmux/quiescence_probe.rs`; Tmux matching gates injection while Pty
  matching resolves readiness outcomes after its pre-wait write.
- Extends the private production-path test with a malformed frame case and
  non-OpenCode preservation coverage.
- No configuration keys, public APIs, dependencies, or persisted data change.
