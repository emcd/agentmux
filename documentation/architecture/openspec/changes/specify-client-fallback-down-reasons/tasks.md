## 1. Verify the corpus gap is still open

- [ ] Confirm no live requirement names `not_started` or `relay_unavailable` as
      a `state_reason_code` value
- [ ] Confirm `bundle-lifecycle` Bundle Down Reason Precedence still scopes
      itself to relay-reported codes

## 2. Verify both surfaces still implement the mapping

- [ ] Confirm `synthesize_unreachable_bundle` in `src/commands/list.rs` derives
      the code from relay-socket presence alone
- [ ] Confirm `synthesize_down_bundle` in `src/mcp/server/handlers/list.rs`
      derives it identically
- [ ] Confirm no third surface synthesizes a down bundle under a different rule

## 3. Cover the contract in tests

- [ ] Add coverage asserting the CLI fallback stamps `not_started` when the
      relay socket is absent and `relay_unavailable` when it is present
- [ ] Add the equivalent coverage for the MCP fallback
- [ ] Assert the two surfaces agree for one filesystem state

## 4. Reconcile the subsystem README

- [ ] Point `src/mcp/README.md` at the requirement as the authority for the
      distinction it currently states on its own
