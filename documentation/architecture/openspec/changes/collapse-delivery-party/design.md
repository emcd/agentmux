## Context

`DeliveryMessage` (in `src/transports/contract.rs`) is the transport-neutral
payload the relay authors after routing/authorization; each transport renders
its own representation from it. Today each party is a `DeliveryParty { session:
String, display_name: Option<String> }`, and coder transports call
`DeliveryParty::to_address()` to obtain an `AddressIdentity` before rendering
pane text. `DeliveryParty` and `AddressIdentity { session_name: String,
display_name: Option<String> }` are structurally identical; `to_address()` is a
field rename (`session` → `session_name`).

`AddressIdentity` is defined in `crate::envelope` — a shared top-level module,
not `crate::relay` — and `contract.rs` already imports it for
`render_pane_envelope`. So carrying it directly introduces no new
transport→relay dependency.

## Goals / Non-Goals

- Goals: carry `AddressIdentity` directly on `DeliveryMessage.sender/target/cc`;
  delete the redundant `DeliveryParty`; preserve the delivery-event parity
  invariant and make it explicit in the specs and a co-landed test.
- Non-Goals: relocating `crate::envelope` / `AddressIdentity` into
  `src/transports` (a separate transport-decoupling move); changing wire
  content of any event or pane header; changing routing/authorization.

## Decisions

- **Decision:** Reuse `crate::envelope::AddressIdentity` directly rather than a
  transports-local identity type. It is a shared envelope type already imported
  by `contract.rs`; a new parallel type would re-create exactly the redundancy
  this change removes. (Q1, confirmed by Coordinator.)
- **Decision:** Add a named non-decorating accessor
  `AddressIdentity::canonical_session_id(&self) -> &str` rather than relying on
  bare `.session_name` field access. The single regression hazard is a future
  `render_address()` swap at the delivery-event call site
  (`src/transports/ui.rs`); a named accessor sitting beside `render_address`
  makes the contrast legible at the call site and gives the spec/test a name to
  bind the invariant to. (Q2, FE's call as ui.rs owner.) Cost is one thin
  borrow-returning method.
- **Decision:** State the pane-envelope exemption as an explicit scenario, not a
  cross-reference. The decorating/non-decorating split is non-obvious; a reader
  must see the exemption stated outright. (Q3, Coordinator.)

## Risks / Trade-offs

- **Risk:** a future edit "upgrades" the incoming_message event fields to
  `render_address`, silently decorating machine-consumed identity fields. →
  Mitigation: the co-landed invariant test asserts the event fields equal the
  bare canonical id, with a fixture whose `display_name` differs from
  `session_name` so the guard bites sharply (`render_address` always wraps).
- **Trade-off:** `canonical_session_id` is a small permanent addition to
  `AddressIdentity`'s public surface. Acceptable under the embeddability
  direction (named, documented accessors are reasonable surface).

## Migration Plan

Internal type change in alpha; no compatibility shims. Replace `DeliveryParty`
construction sites with `AddressIdentity`, delete `DeliveryParty` and
`to_address`, repoint `render_pane_envelope` and `ui.rs` field reads. FE
co-lands the invariant test in the implementation commit.

## Open Questions

- None blocking. (Q1/Q3/Q4 answered by Coordinator; Q2 answered by FE.)
