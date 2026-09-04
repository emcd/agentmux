## MODIFIED Requirements

### Requirement: UI Surface Configuration File

The runtime SHALL support a `ui.toml` operator configuration file for
UI-surface operational defaults, distinct from identity and policy. It SHALL be
resolved at relative path `ui.toml` through the shared effective-file lookup
across the configuration layers, so a copy in an earlier layer shadows a copy in
a later one.

`ui.toml` SHALL be parsed with kebab-case keys. Supported fields SHALL include:

- `default-bundle` (optional): the bundle the TUI browses by default.
- `bindings` (optional): the operator's key bindings, whose contents the
  `tui-binding-configuration` capability governs.

`default-bundle` is a `ui.toml` key; `users.toml` remains the identity and
policy file and does not carry it.

Because layer resolution replaces `ui.toml` whole rather than merging its keys,
a copy supplying `bindings` SHALL supply `default-bundle` as well for that key
to take effect. That coupling is the uniform whole-file rule rather than a
property of the binding group, and the binding group SHALL NOT be given
key-merging semantics across layers to avoid it.

The runtime SHALL treat a missing `ui.toml` as no configured UI-surface
defaults, not an error. Absence from every configuration layer is absence, not a
fault. A malformed `ui.toml` SHALL fail fast with a structured bootstrap
validation error, and a malformed copy in one layer SHALL NOT fall through to a
copy in a later layer. Loading `ui.toml` SHALL be read-only and SHALL NOT
scaffold or modify configuration artifacts. In particular, the default bindings
SHALL be compiled rather than scaffolded into a `ui.toml`, so deleting the file
returns the TUI to its defaults rather than changing what its defaults are.

`agentmux check configuration` SHALL validate `ui.toml` through the same
read-only loader and the same effective-file lookup, reporting a malformed file
with its path and field-level detail before the relay or TUI would reject it at
startup — matching the pre-flight coverage of `relay.toml` and `users.toml`. The
path reported for a malformed file SHALL be the physical file the effective
lookup selected, so an operator can tell which layer is at fault rather than
inspecting a copy that is being shadowed.

#### Scenario: Resolve default browsing bundle from ui.toml

- **WHEN** the runtime resolves the TUI browsing bundle without `--bundle`
- **AND** `ui.toml` defines `default-bundle`
- **THEN** the runtime resolves the browsing bundle from that `default-bundle`

#### Scenario: Earlier layer ui.toml shadows a later one

- **WHEN** `ui.toml` exists under two configuration layers
- **THEN** the copy from the earlier layer supplies the UI-surface defaults
- **AND** the binding group in effect is the one that copy carries, if any

#### Scenario: Treat missing ui.toml as no UI defaults

- **WHEN** no `ui.toml` exists under any configuration layer
- **THEN** the runtime resolves no configured `default-bundle`
- **AND** the compiled default bindings are in force
- **AND** startup proceeds

#### Scenario: Malformed ui.toml does not fall through

- **WHEN** a `ui.toml` in one layer exists but cannot be parsed
- **THEN** loading fails with a structured bootstrap validation error
- **AND** a `ui.toml` in a later layer is not used

#### Scenario: Loading never writes a binding file

- **WHEN** the runtime loads UI-surface configuration
- **THEN** no `ui.toml` is created or modified
- **AND** no default binding is written to disk

#### Scenario: Pre-flight names the layer of a malformed ui.toml

- **WHEN** `agentmux check configuration` reports a malformed `ui.toml`
- **AND** more than one configuration layer supplies a copy
- **THEN** the reported path is the copy in effect rather than any copy it
  shadows
