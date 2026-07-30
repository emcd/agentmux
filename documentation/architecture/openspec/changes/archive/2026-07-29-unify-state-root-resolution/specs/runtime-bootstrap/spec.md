## MODIFIED Requirements

### Requirement: XDG State Root

The system SHALL resolve the state root using, in order:

- the explicit `--state-directory` value when supplied
- `AGENTMUX_STATE_DIRECTORY` when set and non-empty
- `$XDG_STATE_HOME/agentmux` when `XDG_STATE_HOME` is set and non-empty
- `~/.local/state/agentmux` otherwise

The resolved state root SHALL be identical in every build profile. Build profile
SHALL NOT influence state or inscriptions root resolution.

The resolved state root SHALL be normalized to a non-empty absolute path before
use, resolving a relative value against the working directory of the process
performing the resolution. An unnormalized root cannot be propagated: a relative
path re-resolves against each spawned process's working directory, silently
sending a child to a different state root than the relay that spawned it.

`--state-directory` with an empty value SHALL be rejected with a structured
validation error. Empty is not the same signal as absent here: the environment
tier treats blank as absent, so accepting an empty flag would give one spelling
of "nothing" two different meanings depending on which surface carried it.

One state root SHALL correspond to one relay. Isolating a deployment SHALL be
expressed by naming a distinct state root; no deployment identifier is derived
from configuration, build profile, or repository location.

The inscriptions root SHALL continue to default to `<state_root>/inscriptions`,
and therefore follows the state root without separate selection.

A blank `AGENTMUX_STATE_DIRECTORY` SHALL be treated as absent, matching every
other environment tier.

#### Scenario: Resolve state root from XDG variable

- **WHEN** `XDG_STATE_HOME` is set to a non-empty value
- **AND** no explicit or environment state directory is supplied
- **THEN** state root resolves under that directory

#### Scenario: Resolve state root from fallback

- **WHEN** `XDG_STATE_HOME` is unset or empty
- **AND** no explicit or environment state directory is supplied
- **THEN** state root resolves to `~/.local/state/agentmux`

#### Scenario: Environment tier selects the state root

- **WHEN** `AGENTMUX_STATE_DIRECTORY` is set to a non-empty value
- **THEN** state root resolves to that path
- **AND** the XDG and home defaults are not consulted

#### Scenario: Explicit flag outranks the environment tier

- **WHEN** an operator passes `--state-directory`
- **AND** `AGENTMUX_STATE_DIRECTORY` is also set
- **THEN** the state root is the flag's value

#### Scenario: Reject an empty state directory

- **WHEN** an operator passes `--state-directory` with an empty value
- **THEN** the command returns a structured validation error

#### Scenario: Normalize a relative state root

- **WHEN** a relative state root is supplied by flag or environment
- **THEN** it resolves against the working directory into an absolute path
- **AND** that absolute path is what downstream resolution and propagation use

#### Scenario: Build profile does not change the state root

- **WHEN** a debug build and a release build resolve roots from identical
  arguments and environment
- **THEN** both resolve the same state root and the same inscriptions root

### Requirement: Bring-Up Association Environment Injection

Configuration load SHALL stamp authoritative bring-up context into each
coder-backed member's merged spawn environment, so a launched agent propagates it
to its `agentmux host mcp` subprocess and association resolution consults it
rather than inferring identity from the filesystem.

The stamped context SHALL include the hosting bundle name as `AGENTMUX_BUNDLE`
and the member id as `AGENTMUX_SESSION`, and SHALL be extensible to further
context without redefining the mechanism.

Bundle and session context SHALL be stamped upsert-if-absent: an
operator-declared environment entry of the same name SHALL be left untouched.

The relay's normalized state root SHALL additionally be injected as
`AGENTMUX_STATE_DIRECTORY` at spawn time, authoritatively, overwriting any value
already present from coder, bundle, or member configuration. This differs from
bundle and session context deliberately, on two grounds.

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

Generated coder client configuration SHALL NOT emit `--state-directory`. A
template-generated command line is committed content, so a flag in it would
outrank the environment value and silently defeat the rendezvous the injection
exists to guarantee.

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

## REMOVED Requirements

### Requirement: Debug Repository-Local State Override

Build profile no longer selects a state root. The override existed to keep a
source-tree relay from colliding with an installed one, which naming a state root
now does explicitly and in every build profile.
