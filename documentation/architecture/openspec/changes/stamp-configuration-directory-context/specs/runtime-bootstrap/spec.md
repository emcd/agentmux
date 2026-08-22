## MODIFIED Requirements

### Requirement: Bring-Up Association Environment Injection

Configuration load SHALL stamp authoritative bring-up context into each
coder-backed member's merged spawn environment, so a launched agent propagates it
to its `agentmux host mcp` subprocess and association resolution consults it
rather than inferring identity from the filesystem.

The stamped context SHALL include the hosting bundle name as `AGENTMUX_BUNDLE`,
the member id as `AGENTMUX_SESSION`, and the relay's effective configuration
layer list as `AGENTMUX_CONFIGURATION_DIRECTORY`, and SHALL be extensible to
further context without redefining the mechanism.

Bundle, session, and configuration-directory context SHALL be stamped
upsert-if-absent: an operator-declared environment entry of the same name SHALL
be left untouched.

Every name the runtime stamps SHALL be defined in one place with the other
stamped context names, and the set a consumer sanitizes from an inherited
environment SHALL be derived from that definition. A name held apart from it is
a name both sets omit without any consumer failing.

The configuration layer list SHALL be stamped at configuration load rather than
at spawn. Unlike the state root, the value is known at load: it is the layer list
the configuration being loaded was read from.

The effective configuration layer list SHALL be normalized to absolute paths at
root resolution, before it is stamped. A relative layer otherwise re-resolves
against each child's working directory — members routinely declare their own — so
a member would read a different root than the relay that stamped it while both
appear to name the same layer.

Where a layer path contains the separator used by the environment
representation, the list is not faithfully representable in a single environment
value. Configuration load SHALL fail with a structured validation error
identifying the unrepresentable layer when it must serialize the list to stamp
it, and SHALL NOT split the value into fabricated layers or omit the stamp.
Omitting it would return the member to the default tier, which is the condition
the stamp exists to prevent.

The failure SHALL be conditioned on a stamp actually being required, not on a
layer path's contents alone. A coder-less member is never stamped, and a
coder-backed member that declares its own `AGENTMUX_CONFIGURATION_DIRECTORY`
keeps that value under the upsert-if-absent contract, so neither requires the
relay's list to be serialized. A configuration that never needs the
representation SHALL load, and the repeatable command-line flag remains the way
to express such a layer for it.

The relay's normalized state root SHALL additionally be injected as
`AGENTMUX_STATE_DIRECTORY` at spawn time, authoritatively, overwriting any value
already present from coder, bundle, or member configuration. This differs from
bundle, session, and configuration-directory context deliberately, on two
grounds.

First, the value is not known at configuration load. The state root belongs to
the relay performing the spawn, not to the configuration being loaded, so
load-time injection would have to invent or re-derive it.

Second, upsert-if-absent cannot express this contract. A child exists to reach
the relay that spawned it; an operator-declared or blank `AGENTMUX_STATE_DIRECTORY`
would suppress the stamp and send the child to a different relay, which is not an
override of a preference but a broken rendezvous. There is no legitimate reason
for a member of one relay to address another — cross-relay communication is
expressed by configured peers, not by children attaching elsewhere.

The value injected SHALL be the normalized absolute state root, so it does not
re-resolve against the child's working directory.

Spawned coder processes receive the context directly; `agentmux host mcp` is a
descendant of the coder rather than a child of the relay, and receives the
context by ordinary environment inheritance.

Generated coder client configuration SHALL NOT emit `--state-directory` or
`--configuration-directory`. A template-generated command line is committed
content, so a flag in it would outrank the environment value: for the state root
it would silently defeat the rendezvous the injection exists to guarantee, and
for the configuration root it would return the member to reading a root the relay
did not choose.

- The context SHALL be stamped only for coder-backed members; coder-less members
  (`ui`/`pubsub`) spawn no agent and SHALL carry no injected context.
- A blank value SHALL be treated as absent by every consumer, for both
  resolution and any classification derived from presence.

#### Scenario: Stamp context onto a coder member

- **WHEN** a bundle configuration is loaded
- **AND** a coder-backed member declares no `AGENTMUX_BUNDLE`/`AGENTMUX_SESSION`
  environment entries
- **THEN** the member's spawn environment includes `AGENTMUX_BUNDLE` set to the
  hosting bundle name and `AGENTMUX_SESSION` set to the member id

#### Scenario: Preserve operator-declared context

- **WHEN** a coder-backed member explicitly declares an `AGENTMUX_BUNDLE`
  environment entry
- **THEN** configuration load leaves that entry's value untouched

#### Scenario: Skip injection for coder-less members

- **WHEN** a coder-less (`ui` or `pubsub`) member is loaded
- **THEN** its spawn environment carries no injected context entry

#### Scenario: Blank context value is absent at ingress

- **WHEN** a context variable is present in the process environment with a blank
  value
- **THEN** it is normalized to absent where the environment is read
- **AND** every consumer observes it identically as absent

#### Scenario: Inject the state root authoritatively at spawn

- **WHEN** a relay spawns a coder-backed member
- **THEN** the spawn environment carries `AGENTMUX_STATE_DIRECTORY` set to the
  relay's normalized absolute state root

#### Scenario: A configured state directory does not suppress the rendezvous

- **WHEN** a coder, bundle, or member declares `AGENTMUX_STATE_DIRECTORY`,
  whether with a conflicting value or a blank one
- **THEN** the relay's value is injected in its place

#### Scenario: A child stays on the relay that spawned it

- **WHEN** a relay is started with an explicit `--state-directory`
- **AND** it spawns a coder-backed member whose process runs with a working
  directory different from the relay's
- **THEN** the member's `agentmux host mcp` descendant resolves the spawning
  relay's state root
- **AND** reaches that relay's socket rather than the default root's

#### Scenario: Stamp the configuration layer list onto a coder member

- **WHEN** a relay is started with an explicit `--configuration-directory`
- **AND** it spawns a coder-backed member declaring no
  `AGENTMUX_CONFIGURATION_DIRECTORY`
- **THEN** the member's spawn environment carries the relay's effective layer
  list
- **AND** the member's `agentmux host mcp` descendant reads the relay's
  declarations rather than resolving a default root

#### Scenario: A relative configuration layer does not re-resolve in the child

- **WHEN** a relay is started with a relative `--configuration-directory`
- **AND** it spawns a coder-backed member whose process runs with a working
  directory different from the relay's
- **THEN** the stamped value is the layer absolutized against the relay's
  working directory
- **AND** the member resolves the same configuration root as the relay

#### Scenario: A member spawned under a hydration-eligible default no longer scaffolds

- **WHEN** a relay with a resolved configuration root spawns a coder-backed
  member
- **AND** the member's own default configuration root does not exist
- **THEN** the member resolves the stamped root
- **AND** no starter configuration is scaffolded at the member's default root

#### Scenario: An unrepresentable layer list is rejected where a stamp is required

- **WHEN** an effective configuration layer path contains the separator used by
  the environment representation
- **AND** a coder-backed member declaring no `AGENTMUX_CONFIGURATION_DIRECTORY`
  would be stamped
- **THEN** configuration load fails with a structured validation error naming
  that layer
- **AND** no member is spawned carrying a split or omitted layer list

#### Scenario: An unrepresentable layer list loads where no stamp is required

- **WHEN** an effective configuration layer path contains the separator used by
  the environment representation
- **AND** every member is either coder-less or declares its own
  `AGENTMUX_CONFIGURATION_DIRECTORY`
- **THEN** configuration load succeeds
- **AND** each declaring member keeps its own value

#### Scenario: Inherited configuration directory is sanitized

- **WHEN** a consumer clears inherited agentmux context from a process
  environment
- **THEN** `AGENTMUX_CONFIGURATION_DIRECTORY` is cleared with the other context
  variables
- **AND** resolution does not fall through to the clearing process's own
  configuration root
