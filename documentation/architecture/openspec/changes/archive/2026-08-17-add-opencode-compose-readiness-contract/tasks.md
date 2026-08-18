## 1. Contract

- [x] 1.1 Add the proposal, design, and transport-contracts delta for the
  OpenCode compose-region readiness predicate.
- [x] 1.2 Specify the adjacent frame suffix, three-row input region,
  99/100-space boundary, bottommost selection, mismatch semantics, and
  non-OpenCode behavior.

## 2. Implementation

- [x] 2.1 Keep the compose predicate private and apply it after successful
  prompt-regex matching in the Tmux production path.
- [x] 2.2 Require the adjacent info/separator/status suffix instead of
  independent token checks, selecting the bottommost valid suffix.
- [x] 2.3 Keep operational captures under the test-only fixture tree and out
  of the published crate.

## 3. Coverage And Documentation

- [x] 3.1 Cover idle captures, top/middle/bottom compose text, the 99/100/101
  boundary, and a successful non-OpenCode matcher.
- [x] 3.2 Cover OpenCode-looking tokens with a malformed or non-adjacent
  suffix so broad token checks cannot pass unnoticed.
- [x] 3.3 Remove external notebook selectors from code comments and document
  the measured layout constraint in the Tmux subsystem README.

## 4. Verification

- [x] 4.1 Run focused and full nextest, clippy, formatting, package-list, and
  strict OpenSpec validation gates.
- [x] 4.2 Submit the rebased contract-coupled stack to AuxBE for re-review.
