# authorization-scope Specification

## Purpose

Policy presets, authorization vocabulary and evaluation, scope controls, uniform cross-bundle auth, UI sender validation, and per-operation authorization mappings.

## Requirements

### Requirement: Policy Preset Source

Relay authorization policy presets SHALL be loaded from:

- `<config-root>/policies.toml`

`policies.toml` SHALL define presets using `[[policies]]` entries with:

- `id` (required)
- `description` (optional)
- `[controls]` (required)

`policies.toml` MAY define:

- `default` (`<policy-id>`)

Relay SHALL fail fast when this artifact is missing or invalid.

#### Scenario: Reject startup when policies file is missing

- **WHEN** relay starts and `<config-root>/policies.toml` is absent
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with implicit fallback policy

#### Scenario: Reject startup when policies file is invalid

- **WHEN** relay starts and `policies.toml` cannot be parsed or validated
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with partial policy state

#### Scenario: Use built-in conservative default when preset default is absent

- **WHEN** `policies.toml` omits top-level `default`
- **AND** a session omits explicit `policy`
- **THEN** relay applies built-in conservative default policy
- **AND** built-in controls are:
  - `find = self`
  - `list = home`
  - `look = self`
  - `send = home`
  - `do` defaults to `none` for unspecified actions

### Requirement: Session Policy Binding

Each session definition SHALL support optional binding to a policy preset id:

- `policy = "<policy-id>"`

If session `policy` is omitted, relay SHALL resolve policy by precedence:

1. top-level `default` preset in `policies.toml` when present
2. built-in conservative default policy

Relay SHALL reject bundle configuration when a session references an unknown
policy id.

#### Scenario: Reject unknown session policy reference

- **WHEN** a session declares `policy = "missing-policy"`
- **AND** `policies.toml` has no matching `[[policies]].id`
- **THEN** relay rejects configuration with a validation error

#### Scenario: Resolve omitted session policy from top-level default

- **WHEN** session omits explicit `policy`
- **AND** `policies.toml` defines top-level `default`
- **THEN** relay uses that default policy preset for the session

### Requirement: Authorization Control Vocabulary

Relay SHALL evaluate authorization using canonical controls and scope values:

- `find`: `none` | `self` | `home` | `all`
- `list`: `none` | `self` | `home` | `all`
- `look`: `none` | `self` | `home` | `all`
- `send`: `none` | `self` | `home` | `all`
- `do`: map `action_id -> (none | self | home | all)`

The policies file is authoritative: every control accepts the full scope
ladder at parse time, and the consuming authorization checks give each value
its effect via scope rank order.

For current self-target-only `do` behavior:

- `none` and `self` are operative
- `home` and `all` are reserved/non-operative until non-self `do`
  targeting is introduced

#### Scenario: Evaluate look request using configured look scope

- **WHEN** relay evaluates a `look` request
- **THEN** it uses the session policy control `look`
- **AND** applies one of the canonical scope values

#### Scenario: Treat missing do action entry as none

- **WHEN** relay evaluates `do` authorization
- **AND** requested action id is not present in `do` control map
- **THEN** relay treats authorization scope as `none`

#### Scenario: Treat do all-home/all-all scopes as reserved

- **WHEN** relay evaluates `do` authorization
- **AND** action scope is `home` or `all`
- **THEN** relay treats scope as reserved/non-operative
- **AND** non-self `do` execution remains unsupported by runtime contract

### Requirement: Centralized Authorization Decision Point

Relay SHALL be the centralized authorization decision point.
CLI and MCP SHALL remain validators/adapters and SHALL NOT implement
independent authorization decision logic.

#### Scenario: Return relay-authored denial across surfaces

- **WHEN** a request is denied by policy
- **THEN** relay returns canonical denial response
- **AND** CLI/MCP propagate that denial without re-evaluating authorization

### Requirement: Authorization Evaluation Order

Relay SHALL evaluate requests in this order:

1. request validation
2. requester identity resolution
3. bundle/target/action resolution
4. authorization policy evaluation
5. execution

Validation failures SHALL take precedence over authorization denials.

#### Scenario: Prefer validation failure over authorization denial for non-send target

- **WHEN** a non-send request includes an unknown target session
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

#### Scenario: Prefer send explicit-target validation over authorization denial

- **WHEN** a send request includes an unknown or non-canonical explicit target
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

### Requirement: Authorization Denial Schema

When relay denies a valid/resolved request by policy, relay SHALL return
`authorization_forbidden` with `details` including:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

#### Scenario: Return canonical denial details for single-target operation

- **WHEN** relay denies a same-bundle non-self look request by policy
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include required fields
- **AND** denial details include `target_session`

### Requirement: Relay List Authorization

Relay `list_sessions` responses SHALL require policy evaluation for capability
`list.read`.
If requester identity is valid and list access is denied by policy, relay SHALL
return `authorization_forbidden` and SHALL NOT return successful list payload.

The successful list payload collection key SHALL be `principals[]` on the
canonical `ListedBundle` payload (renamed from `sessions[]`); the per-entry
`ListedSession` shape is unchanged.

#### Scenario: Deny list_sessions without successful payload

- **WHEN** requester identity is valid
- **AND** policy denies `list.read` for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return a successful `bundle.principals[]` payload

### Requirement: Relay Send Scope Control

Relay send authorization SHALL be driven by `send` control scope, evaluated
against the requester's dispatch (home) bundle policy:

- `home` allows only same-bundle targets
- `all` allows cross-bundle targets

Cross-bundle send SHALL require `all`; a cross-bundle send issued under
`home` SHALL be rejected with `authorization_forbidden`.

#### Scenario: Reject cross-bundle send under home-only scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "home"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Permit cross-bundle send under all-all scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all"`
- **THEN** relay routes and delivers to the cross-bundle target(s)

### Requirement: Authorization Hooks for Do and Find

Relay SHALL reserve authorization hooks for:

- `do` action-id scoped controls
- `find` scope controls

These hooks SHALL use the same evaluation order and denial schema as `list`,
`send`, and `look`.

#### Scenario: Deny do action run with canonical schema

- **WHEN** relay denies action execution by `do` control map
- **THEN** relay returns `authorization_forbidden`
- **AND** details include canonical required fields

#### Scenario: Deny do action run when do map sets none

- **WHEN** requested action id maps to `none` in `do` control map
- **THEN** relay returns `authorization_forbidden`

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's identity and policy controls in the
requester's **home namespace** — its bound bundle's policy for a session
principal, or the operator policy for a relay-wide principal — classify the
requester-to-target relationship relative to that home namespace, and require a
scope tier on the policy scope ladder:

- self target -> `self`
- same-namespace non-self target -> `home`
- other-namespace target -> `all`

A principal's home namespace SHALL be its native namespace: a session's home is
its bundle, and a relay-wide principal's home is its reserved namespace
(`GLOBAL` / `EXTERNAL` / `RELAY`). `home` SHALL therefore confer authority only
within the principal's own namespace; a relay-wide principal (for example a
`@GLOBAL` operator) SHALL require `all` to reach into any bundle, since a bundle
is not its home namespace. There SHALL be no global/relay-principal exemption
from this threshold.

This requester-axis rule has a target-axis counterpart: a relay-wide (`@GLOBAL`)
*target* SHALL classify at the `home` tier rather than `all`, because it is
delivered through the unified registry in its own namespace rather than by
crossing into a peer bundle. Reaching a relay-wide target — an agent messaging
the operator, or one relay-wide principal messaging another — is therefore not a
cross-namespace act and SHALL NOT demand `all`. This is a routing invariant, not
a per-operation policy exemption.

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-namespace policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer namespace
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer namespace SHALL NOT be required, and the
relay SHALL NOT resolve or authorize the requester in a target's (or any other
borrowed) bundle in place of its home namespace, on any target operation.

Whether a capability can be configured to a cross-bundle (`all`) scope SHALL be
governed by the policy schema's per-capability allowed-scope set, not by relay
routing code. A capability whose schema cap is below `all` SHALL therefore be
unreachable cross-bundle until the policy schema is widened, with no code
override involved.

#### Scenario: Requester authorized in dispatch bundle, not peer bundle

- **WHEN** a session in bundle A issues a cross-bundle operation targeting
  bundle B
- **THEN** relay evaluates the requester's policy controls from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-namespace session raww/look authorizes in the home namespace

- **WHEN** a session in bundle A issues a `raww` or `look` targeting a session in
  bundle B
- **AND** the requester's configured scope for that capability in bundle A's
  policy is `all`
- **THEN** relay resolves and authorizes the requester in bundle A (its home
  namespace) and the operation succeeds against the bundle B target
- **AND** relay does not resolve the requester in bundle B and does not return
  `validation_unknown_sender` for a requester unknown there

#### Scenario: Cross-bundle operation denied under home scope

- **WHEN** a requester issues a cross-bundle `look`, `send`, or `list`
- **AND** the requester's configured scope for that capability is `home` or
  narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Cross-bundle list enumerates peer bundle under all-all scope

- **WHEN** a requester with `list = all` lists a configured peer bundle's
  sessions
- **THEN** relay returns the peer bundle's session listing rather than rejecting
  the requester as unknown

#### Scenario: Relay-wide principal needs all-all to reach a bundle

- **WHEN** a relay-wide principal (for example a `@GLOBAL` operator) issues a
  `list` or `send` targeting a bundle namespace
- **AND** its configured scope for that capability is `home`
- **THEN** relay returns `authorization_forbidden`, because the bundle is not the
  principal's home (`GLOBAL`) namespace
- **AND** the same principal under `all` is permitted

#### Scenario: Capability not configurable to cross-bundle scope fails uniformly

- **WHEN** a requester issues a cross-bundle request for a capability whose
  policy-schema cap is below `all`
- **THEN** the request fails the uniform `all` threshold with
  `authorization_forbidden`
- **AND** no operation-specific code override is involved

### Requirement: UI Request-Path Sender Validation

Relay SHALL validate non-hello request-path UI sender identities using global
TUI sessions from `<config-root>/users.toml`.

For request-path operations such as `send`, relay SHALL:

1. validate sender `session_id` exists in global TUI sessions,
2. evaluate authorization using that TUI session's `policy` reference,
3. return canonical `authorization_forbidden` when policy denies.

#### Scenario: Authorize send using global UI session policy

- **WHEN** relay receives `send` request with UI sender `session_id = "user"`
- **AND** global TUI sessions include `id = "user"` with `policy = "ui-default"`
- **THEN** relay evaluates authorization using policy `ui-default`

#### Scenario: Reject request-path sender missing from global UI sessions

- **WHEN** relay receives `send` request with UI sender `session_id = "ghost"`
- **AND** no global TUI session maps to `id = "ghost"`
- **THEN** relay rejects request with `validation_unknown_sender`

### Requirement: Relay List Sessions Request Scope

Relay SHALL support only single-bundle session listing requests.
Relay SHALL NOT accept all-bundle list selectors.

#### Scenario: Reject all-bundle relay list selector

- **WHEN** a caller requests relay list with all-bundle selector semantics
- **THEN** relay rejects request with `validation_invalid_params`

### Requirement: Relay raww authorization mapping

Relay SHALL evaluate raww authorization using policy control `raww`.

Policy scope contract:
- allowed values: `none`, `self`, `home`, `all`
- invalid values (unknown values) SHALL fail configuration validation with
  `validation_invalid_policy_scope`

When raww is denied by policy, relay SHALL return `authorization_forbidden`
with canonical minimum details:
- `capability` = `raww.write`
- `requester_session`
- `bundle_name`
- `reason`

#### Scenario: Deny raww under self scope for non-self target

- **WHEN** requester policy sets `raww = "self"`
- **AND** requester invokes raww to another session in the same bundle
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

#### Scenario: Cross-bundle raww permitted under all

- **WHEN** requester policy sets `raww = "all"`
- **AND** requester invokes raww to a session in a different bundle
- **THEN** relay routes to the target and delivers
