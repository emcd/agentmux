# Design: Unified namespace-keyed session registry

## Context

`add-relay-routing-layer` made target routing namespace-centric: qualified
principal IDs (`session@namespace`) determine target namespace, and authorization
uses the requester's home namespace. The stream registry still reflects the older
bundle-vs-relay-wide split: bundle sessions key by `(bundle_name, session_id)`,
while relay-wide sessions key by `principal_id` through `RegistryKey::RelayWide`.

The registry split leaks into operation bodies. `GLOBAL` list bypasses the normal
list path, look/raww resolve relay-wide capabilities from global configuration,
and send/delivery carry relay-wide booleans to choose a different lookup path.

## Goals / Non-Goals

- Goals: unify session keying on canonical `principal_id`; make namespace a
  registry entry attribute; populate entries for static bundle sessions and
  dynamic streams; store per-entry transport capability flags; remove relay-wide
  registry special paths; keep routing and authorization semantics unchanged.
- Non-Goals: changing relay request/response JSON shapes, changing policy scope
  semantics, introducing target-side ingress ACLs, implementing new transport
  types, or changing bundle lifecycle behavior.

## Decisions

### Decision: canonical `principal_id` is the registry key

The session registry uses one `HashMap<String, RegistryEntry>` keyed by
canonical `principal_id`. The key always includes an `@<namespace>` suffix.
Bundle sessions use `session@bundle`; relay-wide sessions use `session@GLOBAL`.
`EXTERNAL` and `RELAY` remain reserved namespaces and are not introduced as
deliverable targets by this change.

The entry stores parsed attributes so callers do not repeatedly split strings:
bare session id, namespace, principal class, registration source, optional
bundle binding, transport/runtime binding, stream writer when connected, and
authenticated identity. `GLOBAL` is a namespace value, not a different registry
kind.

### Decision: namespace is the registry vocabulary

The unified registry uses `namespace` for every namespace value because bundle
names, `GLOBAL`, `EXTERNAL`, `RELAY`, and future reserved namespaces flow through
the same keying model. `bundle_name` remains appropriate only for fields or
operations that specifically target configured session bundles.

### Decision: each entry carries transport capabilities

Each registry entry stores transport capability flags derived from its transport
type when the entry is created:

- `can_be_looked`
- `can_be_written`
- `can_stream_output`
- `can_give_choices`

The flags are the target-side operation capability source for configured and
connected targets. This incorporates the companion transport-capability design
into the registry shape so the unified registry does not need a follow-on
retrofit. If capability attribute work lands as a separate proposal, it should
use the same entry fields and naming rather than reintroducing per-operation
derivation paths.

### Decision: registration source is metadata, not key shape

Bundle sessions and relay-wide UI sessions differ in how their entries are
created and evicted, not how they are addressed. Bundle runtime startup creates
static entries for configured coder sessions with their transport type,
capability flags, runtime directory, and delivery binding. Stream hello creates
or attaches dynamic stream state such as writer and revoke signal. A registry
entry records its source so lifecycle helpers can filter by namespace/source when
needed.

Bundle reload/unload eviction matches entries whose namespace is the affected
bundle. Credential revocation matches entries by authenticated identity. These
remain selector predicates over one registry rather than separate code paths.

### Decision: target resolution returns registry entries

Operation bodies use a shared target-entry resolver after routing and before
authorization. The resolver accepts a qualified principal ID, finds the registry
entry by that exact key, and exposes the entry's transport/runtime binding and
capability flags. Look and raww apply capability gates from the entry before
policy authorization, preserving existing validation precedence.

Configured-but-not-ready sessions remain distinguishable where existing behavior
requires it: static registry entries can exist without a connected stream writer
or ready runtime, and operation bodies preserve today's unavailable/stale/queued
behavior instead of collapsing that state into `validation_unknown_target`. The
registry is the authority for target identity, capabilities, and delivery binding
shape; readiness remains a per-transport/runtime state.

Removing `relay_wide_target` has delivery-path consequences: worker and payload
code must derive stream-delivered versus coder-delivered behavior from the
registry entry's transport binding instead of from a boolean copied through the
route.

### Decision: `GLOBAL` list uses normal namespace listing

List for `namespace = "GLOBAL"` enumerates registry entries whose namespace is
`GLOBAL`. It no longer bypasses request handling through a dedicated relay-wide
list function or matches `RegistryKey::RelayWide`. The synthetic bundle-shaped
list response is preserved so clients see the same payload shape.

Bundle-subject operations such as `up`, `down`, and choice decisions are
intentionally unchanged. They still resolve configured bundles through the
existing namespace routing path; this proposal changes session target lookup,
not bundle lifecycle command routing.

## Risks / Trade-offs

- Keying by canonical string centralizes correctness on canonicalization. The
  implementation should canonicalize once at registration and reject unqualified
  or non-canonical live entries.
- Configuration changes can leave pre-existing live entries with the capability
  flags they registered with. Bundle reload/unload should evict affected entries
  so reconnect/startup recreates them from the current configuration.
- Configured-vs-ready target semantics must stay explicit. The proposal unifies
  session registry lookup, not the persistent bundle/user configuration stores.

## Migration Plan

1. Add the unified registry entry type and canonical-principal key helpers.
2. Populate static bundle-session entries during runtime startup and dynamic
   stream state during hello registration.
3. Replace relay-wide/bundle lookup helpers with unified entry lookup and
   namespace-filtered enumeration.
4. Update list, send, look, raww, revocation, and bundle eviction call sites.
5. Remove `RegistryKey`, `RegistryKey::RelayWide`, relay-wide target booleans,
   and stale comments that describe the split registry.
