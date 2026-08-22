## Why

The relay never propagates its configuration root to the members it spawns. A
member therefore resolves configuration independently of the relay that started
it: it falls through to `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`, and
because only the default tier permits hydration, a member whose default root does
not exist is scaffolded into a fresh, empty, apparently-working deployment rather
than failing. Operators work around this by naming
`--configuration-directory` in every generated client configuration, which makes
a working deployment depend on per-worktree overrides instead of on the committed
configuration.

The same omission leaks into the test suite. A harness that clears inherited
agentmux context does not clear `AGENTMUX_CONFIGURATION_DIRECTORY`, because the
sanitization list does not name it, so a developer with that variable exported
runs suites against their own configuration root.

Both follow from one cause. Every other context variable name is defined in
`configuration/types.rs`, whose comment states that holding a name elsewhere is
how the list silently omits it; the configuration-directory name is defined
elsewhere, privately, in `runtime/paths.rs`. The stamping set and the
sanitization set are both built from the list that cannot see it.

## What Changes

- Relocate the configuration-directory environment variable name into
  `configuration/types.rs` alongside the other context names, so both derived
  lists can see it.
- Stamp `AGENTMUX_CONFIGURATION_DIRECTORY` onto each coder-backed member's merged
  spawn environment at configuration load, **upsert-if-absent**, as ordinary
  bring-up context. This is not a second authoritative exception: the state-root
  exception exists because that variable names the relay a member is a child of,
  and a wrong value breaks the rendezvous. Socket, session and peer PSKs, and the
  principal store all resolve under the state root, so a divergent configuration
  root does not misroute or misauthenticate a child — it gives it a different set
  of declarations. The environment tier also ranks below the CLI flag, so an
  authoritative guarantee would be unenforceable.
- Add the variable to the inherited-context sanitization set, closing the
  test-isolation leak.
- **BREAKING**: normalize the effective configuration layer list to absolute
  paths at root resolution. Explicit and environment tiers currently pass their
  declared paths through unnormalized, and a relative layer resolves against the
  process working directory at lookup time. That behavior is documented today and
  is incompatible with propagation: a member that declares its own working
  directory would resolve a stamped relative layer against its own directory
  rather than the relay's. The state root is already normalized for exactly this
  reason.
- Emit a structured validation error when an effective layer list cannot be
  represented in the separator-delimited environment value — that is, when a
  layer path contains the separator. Silently splitting the value would fabricate
  layers; silently omitting the stamp would return the member to the default tier,
  which is the defect being fixed.
- Forbid generated coder client configuration from emitting
  `--configuration-directory`, mirroring the existing prohibition on
  `--state-directory`. A committed flag outranks the environment tier and would
  defeat the stamp in precisely the deployment path this change exists to fix.

Members move from the default configuration tier to the environment tier as a
result, and so are no longer eligible for hydration. That is the intended
outcome, not a side effect: the scaffolding it removes is the silent-success
failure described above.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `environment-variables`: the Environment Variable Precedence requirement
  enumerates `AGENTMUX_STATE_DIRECTORY` as "the one exception" to upsert-if-absent
  stamping and reads as exhaustive about what is stamped. It gains
  configuration-directory as ordinary stamped context, and states why that
  variable does not qualify for the exception.
- `runtime-bootstrap`: the Bring-Up Association Environment Injection requirement
  enumerates the stamped context set, which the precedence requirement delegates
  to it. It gains the configuration layer list as stamped context, the
  absolute-path normalization guarantee, the unrepresentable-list error, and the
  prohibition on generated configuration emitting `--configuration-directory`.

## Impact

Code: `src/runtime/paths.rs` (const relocation, layer normalization at
resolution), `src/configuration/types.rs` (context name, `BringUpContext` field,
both name lists), `src/configuration/loaders.rs` (stamping),
`src/relay/lifecycle.rs` (supplying the resolved layer list to load).

Behavior: a relative `--configuration-directory` or
`AGENTMUX_CONFIGURATION_DIRECTORY` value is absolutized against the relay's
working directory at resolution rather than re-resolved at each lookup.
Deployments that rely on lookup-time re-resolution are affected.

Documentation: `src/runtime/README.md` is the subsystem architecture
documentation for these tiers; its Root Resolution section presents the
repeatable flag as the escape hatch for a separator-containing path, which this
change makes incomplete, and documents normalization for the state root alone.
`documentation/usage/maintainer-configuration-guide.md` documents the current
normalization asymmetry as intended and must be corrected in the same change;
`documentation/usage/operations.md` for the operator-facing consequence.

Committed client configuration: two of the three in-repo artifacts carrying an
`agentmux host mcp` command line emit `--configuration-directory` today. They are
corrected here, and `scripts/lint-client-configuration.sh` — which already
enforces the identical prohibition on `--state-directory` — is extended to keep
them corrected.

Dependency outside this repository: those artifacts are generated from templates
in the shared agent tooling distribution. Correcting the copies here does not
correct the templates, so the next template update would reintroduce the flag.
The extended lint turns that into a caught failure rather than a silent
regression, but the template still has to follow for template-generated
deployments elsewhere to benefit.
