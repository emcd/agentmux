## 1. Verify the corpus gap is still open

- [x] Confirm no live requirement names `not_started` or `relay_unavailable` as
      a `state_reason_code` value
- [x] Confirm `bundle-lifecycle` Bundle Down Reason Precedence still scopes
      itself to relay-reported codes

## 2. Verify both surfaces still implement the mapping

- [x] Confirm `synthesize_unreachable_bundle` in `src/commands/list.rs` derives
      the code from relay-socket presence alone
- [x] Confirm `synthesize_down_bundle` in `src/mcp/server/handlers/list.rs`
      derives it identically
- [x] Confirm no third surface synthesizes a down bundle under a different rule

## 3. Cover the contract in tests

Not done, and deliberately not blocking the archive. These would assert
behavior both surfaces already have; the requirement documents an existing
contract rather than introducing one, so the specification is complete without
them. Carried forward as `agentmux:todos/openspec/18` so the gap is tracked
rather than forgotten.

- [ ] Add coverage asserting the CLI fallback stamps `not_started` when the
      relay socket is absent and `relay_unavailable` when it is present
- [ ] Add the equivalent coverage for the MCP fallback
- [ ] Assert the two surfaces agree for one filesystem state

## 4. Reconcile the subsystem README

Not done. `src/mcp/README.md` states the distinction correctly on its own, so
this is a cross-reference rather than a correction, and it edits a file the
relay lane owns. Carried forward with the test coverage above.

- [ ] Point `src/mcp/README.md` at the requirement as the authority for the
      distinction it currently states on its own
