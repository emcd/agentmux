## Why

Two requirements in `authorization-scope` contradict each other, and the
implementation has already ruled against one of them.

`Authorization Control Vocabulary` states that the policies file is
authoritative: every control accepts the full `none`/`self`/`home`/`all` ladder
at parse time, and consuming checks give each value its effect by rank order.

`Uniform Cross-Bundle Authorization Model` states the opposite: that whether a
capability may be configured to `all` is governed by a per-capability
allowed-scope set in the policy schema, and that a capability whose cap is
below `all` is unreachable cross-bundle until the schema is widened.
`relay-routing-layer`'s `Authorization Stage` repeats that claim and names
`parse_policy_controls` as its authority.

No such cap exists. `parse_policy_controls`
(`src/relay/authorization/loading.rs:247`) parses controls and implements no
cap, and the parser carries a guard against reintroducing one:

    // The policies file is authoritative: every control accepts the full
    // none/self/home/all ladder, and consuming authorization checks give each
    // value its effect via rank order. Do not reintroduce per-control
    // allowed-scope caps here.

This is worse than a stale name. One scenario describes behavior the
implementation deliberately refuses to have — its `WHEN` clause names a
capability whose policy-schema cap is below `all`, and no capability has a
schema cap, so the scenario can never be exercised. A reader implementing to
the specification would build the mechanism the code forbids.

It also blocks planned work. Distributing verb authorizability out of
`authorization-scope` into the capability owning each verb would propagate this
contradiction into every destination, so it has to be resolved first.

## What Changes

The cap is described in four places and asserted in none of the code. All four
go; the correct rule already exists in `Authorization Control Vocabulary` and
needs no restatement.

- `authorization-scope`, `Uniform Cross-Bundle Authorization Model`: delete the
  paragraph governing cross-bundle reach by a policy-schema cap. The preceding
  paragraph already states the operative rule — reach is determined solely by
  the requester's configured scope against the uniform threshold.
- `authorization-scope`, same requirement: delete the scenario
  `Capability not configurable to cross-bundle scope fails uniformly`, whose
  `WHEN` clause is unsatisfiable.
- `relay-routing-layer`, `Authorization Stage`: strike the cap sentence while
  keeping the rule it was attached to, that the relay applies no per-operation
  cross-namespace logic in handler or routing code.
- `relay-routing-layer` Purpose preamble and `src/relay/routing.rs` module
  documentation: both assert the cap in prose. Neither is delta-governed.

No behavior change. Nothing in `src/` implements the cap, so no code is being
made to match the specification; the specification is being made to match the
code.

## Capabilities

### Modified Capabilities

- `authorization-scope`: `Uniform Cross-Bundle Authorization Model` no longer
  makes cross-bundle reach conditional on a per-capability schema cap, and
  loses the scenario that could not be exercised.
- `relay-routing-layer`: `Authorization Stage` no longer names a schema
  allowed-scope set as the authority for cross-namespace reach.

## Impact

Specification and documentation only.

- `documentation/architecture/openspec/specs/authorization-scope/spec.md`
- `documentation/architecture/openspec/specs/relay-routing-layer/spec.md`
- `src/relay/routing.rs` — module documentation comment, no code

Unblocks the distribution of verb authorizability out of `authorization-scope`
(`todos/openspec/5`).

Deliberately out of scope: `Authorization Hooks for Do and Find`, which was
originally filed alongside this. That requirement's `SHALL reserve` is
satisfied — both controls are parsed and deliberately discarded at
`src/relay/authorization/resolution.rs:182-183` — and removing the reservation
would be a breaking configuration change rather than a hygiene edit, since
`RawPolicyControls` carries `deny_unknown_fields` and `find` is a required key
that the shipped `data/configuration/policies.toml` sets. Tracked separately as
`todos/openspec/11`.
