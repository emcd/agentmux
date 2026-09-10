## Why

Setting up a peer relay today forces operators to choose between exposing a generated PSK in command output (generic `--output` needs a pre-existing parent directory) and hand-crafting relay-owned credential directories themselves — while the one safe-looking helper, `--write-config`, rejects relay principals outright (`agentmux:issues/relay/84`). Concretely: an operator running two relays on the same host (alpha and bravo) who wants bravo to authenticate against alpha must either copy a PSK out of a `new peer` response or pre-create the peer credential directory by hand. Both flows make the operator responsible for filesystem placement the relay could safely do itself. At the same time, the credential-write path that `--write-config` does use confines only the final temporary file with `O_NOFOLLOW` while `create_dir_all`, permission changes, temp creation, rename — and the principal-store load/persist beneath the same state root — all follow symlinked ancestor directories, so a symlink beneath the state tree can redirect a credential write outside the physical state tree (`agentmux:issues/relay/57`). Provisioning must therefore close the confinement gap rather than route around it: the new flow has to be safe by construction.

## What Changes

- Add a dedicated peer-link flow with a fully specified two-relay protocol. The one-way primitive (register alias referent, issue inbound identity, install into the peer slot) explicitly discards the registration mint; the paired bidirectional algorithm — the only fresh bidirectional algorithm — verifies declared symmetric naming BEFORE either mint, retains both alias mints, and cross-installs them with zero drops. An existing one-way link upgrades via rotation of the discarded alias credential plus one install. Exactly one flow; `new peer` / `change psk` Config semantics stay session-only and unchanged.
- The coordinator addresses both relays by explicit state roots (sockets derived as `<state-root>/relay.sock`, both relays live); `[[peers]]` entries are added after linking, not before. Owner-only permissions throughout (0700 dirs, 0600 files); unsafe path components refused, never followed.
- The secret-transfer boundary is explicit: coordinator memory, the issuance Response payload, and the install secret argument may carry the PSK; rendering, logs, files (other than the destination slot), snippets, diagnostics, and adapter-written persistence never may. Split MCP operation exposes the PSK to the MCP client by construction and operators choosing it accept that exposure.
- Harden every state-root-owned credential write — sink operations for both credential commands AND principal-store load/persist — with symlink-safe traversal anchored on retained directory handles; ancestor-symlink rejection is tested, including ancestor exchange after staging. Install authorization classifies absent, identical, and different slot content under one lock so lost-response retry stays safe for the original caller; per-step timeouts (registration, issuance, install) each have specified forward recovery; install durability is temp-write plus file fsync, atomic rename, then directory fsync with typed uncertainty past the rename point.
- Explicitly NOT changing: generic `--output` parent creation stays refused; Response/Path destination semantics stay as specified (Path keeps its final-target symlink check only); `agentmux:issues/relay/83`'s end-to-end setup guide remains separate documentation work. No cross-relay transaction: partial completion has specified forward-recovery paths, not automatic compensation.

## Capabilities

### New Capabilities

- `peer-credential-provisioning`: the coordinated peer-link protocol — bootstrap order, per-side authorization, alias/connect-as mapping, secret-transfer channel, idempotent install, and partial-failure recovery; covers CLI coordination plus the new MCP install surface and relay behavior.
- `credential-path-confinement`: symlink-safe traversal/confinement for state-root-owned credential writes — sink operations, store load/persist, retained-dirfd semantics, ancestor-symlink rejection with a pinned error code.

### Modified Capabilities

- `mcp-tool-surface`: ADDED peer-link install tool contract (new `link` meta-tool surface for installing a peer credential into the connected relay's relay-owned slot). The existing New/Change destination contracts are unchanged.

## Impact

- `src/relay/identity.rs` (`stage_credential_sink`, `write_pending_credential`, `PendingCredentialWrite::commit`, `PrincipalStore` load/persist), `src/relay/handlers/identity.rs` (new install operation plus existing flows' rollback paths).
- New CLI `link peer` coordination; new MCP `link` meta-tool; shared traversal helper (new module or `src/runtime/paths.rs`).
- Unit + integration tests for ancestor-symlink rejection (sink and store), ancestor exchange, install idempotency/recovery, and the link flow on both surfaces.
