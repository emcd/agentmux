## Context

Credential provisioning for relay peers has two gaps that must be closed together (`agentmux:issues/relay/57`, `agentmux:issues/relay/84`): no safe provisioning flow, and symlink-following traversal in the write paths.

The data model constrains the protocol (see `runtime-bootstrap`'s Outbound Peer Relay Configuration and `cross-relay-routing`'s Outbound Peer Connection Management). On the destination relay B, for peer A:

- B's `[[peers]]` entry holds `alias` (B's local name for A), `address` (A's socket), and `connect-as` (the bare id A issued B). `alias` and `connect-as` are independent; nothing keys on `connect-as`.
- B must register `<alias>@RELAY` locally via `new peer` — the registration gives the alias a referent and carries the inbound scope A may reach. Startup validation requires the record's existence and relay type.
- B's outbound credential file is `peers/<alias>.psk` (mode 0600): the raw PSK **A issued B**. A holds only the hash on its `<connect-as>@RELAY` record. The two directions never agree by construction.

Registration via `new peer` necessarily mints: every alias registration produces a PSK the coordinator must account for — retain (paired mode) or explicitly discard (one-way mode). There is no mint-less registration, so the protocol handles both cases normatively rather than leaving drops implied.

Stakeholders: operators linking same-host or cross-host peer relays; the relay's sink, store, and install code; CLI/MCP adapter surfaces.

## Goals / Non-Goals

Goals:

- One dedicated peer-link flow with a normative protocol: one-way primitive plus paired bidirectional algorithm, endpoint selection, per-side authorization with retry-safe classification, per-step timeout recovery, install durability, and an explicit secret-transfer boundary.
- Symlink-safe traversal for every state-root-owned credential write: sink operations for both credential commands AND principal-store load/persist, anchored on retained directory handles.
- Owner-only permissions on everything the flow creates (0700 dirs, 0600 files).

Non-Goals:

- Generic `--output` parent creation (stays refused); Response/Path semantics (unchanged; Path keeps its final-target symlink check only).
- Relay-to-relay provisioning push (no trust channel exists for it; the operator's client coordinates).
- Opaque sealed transfer / relay-held escrow (rejected in decision 5; the explicit boundary below covers the threat model without new secret-bearing surface).
- `[[peers]]` entry management (operator-owned config; the protocol states the post-condition, existing startup/delivery validation enforces it).
- `agentmux:issues/relay/83` end-to-end setup guide (separate documentation work).
- PSK cryptography, hashing, revocation semantics.

## Decisions

### 1. One dedicated peer-link flow; existing Config semantics untouched

Exactly one flow: the dedicated peer-link flow. `new peer` / `change psk` `--write-config` keeps its session-only meaning because those commands' arguments cannot name another relay, another state root, or a destination alias.

### 2. Client-coordinated protocol with normative endpoint selection; entries after linking

No authenticated relay-to-relay channel exists for provisioning directives. The coordinator takes two state roots, derives each relay's socket as `<state-root>/relay.sock` (the `RelayRuntimePaths` mapping), and requires both relays live and reachable. Neither relay needs the other's `[[peers]]` entry yet — entries are operator-managed config added AFTER linking, because startup validation fails on entries whose alias record does not exist yet. This resolves the bootstrap cycle: link first against running relays, add entries, subsequent startup (and first delivery) validates. The required post-condition is that each entry's `connect-as` equal the opposite relay's alias; a mismatch surfaces as an authentication failure at first delivery under existing validation, and the setup guide (out of scope) documents it.

### 3. One-way primitive with explicit mint-discard; paired bidirectional algorithm

**One-way** (B dials A): register `<alias>@RELAY` on B → issue `<connect-as>@RELAY` on A (Response destination) → install into B's `peers/<alias>.psk`. The registration mint is explicitly discarded by the coordinator: dropped from memory without rendering, logging, or persisting. The alias record's credential remains valid-but-unheld, subject to the normal rotation lifecycle (a later `change psk` may put it into someone's hands exactly as for any registered principal) — discarding is safe because no party needs that credential for this direction.

**Paired bidirectional** (single invocation, both directions — the ONLY fresh bidirectional algorithm): the symmetric naming the operator declares (A's `connect-as` equals B's `alias` and vice versa) makes the two alias mints exactly the two needed credentials — nothing is dropped. Declared cross-equality is verified FIRST, before either mint, because both values are known upfront and checking after two mints would needlessly leave two records and secrets on mismatch:

1. Verify declared cross-equality (each side's declared `connect-as` equals the opposite alias); abort on mismatch with nothing minted. This verifies operator-stated intent; it derives nothing, preserving the live-spec independence of alias and connect-as.
2. Register `<alias-B>@RELAY` on B, coordinator retains K1.
3. Register `<alias-A>@RELAY` on A, coordinator retains K3.
4. Install K1 into A's `peers/<alias-A>.psk` (A presents `<alias-B>@RELAY` to B, verified against B's record).
5. Install K3 into B's `peers/<alias-B>.psk` (B presents `<alias-A>@RELAY` to A, verified against A's record).

Two mints, two installs, zero drops, no issuance step, no second-claim hazard (all records are fresh). Reciprocal live configuration necessarily cross-equals, so two independent one-way invocations CANNOT compose into a bidirectional link (the reverse direction's records already exist and their mints were discarded) — the composition claim is removed.

**One-way to bidirectional upgrade** (existing one-way B→A plus desired reverse): under reciprocal naming the one-way invocation already created A's `<alias-A>@RELAY` issuance record (as `connect-as`), so only A's outbound file is missing. Rotate B's `<alias-B>@RELAY` record via `change psk` (fresh known K1', `change.psk=all` on B) and install K1' into A's `peers/<alias-A>.psk`. One rotation plus one install; no re-registration.

### 4. Install authorization classifies absent, identical, and different under one lock

Slot-state authorization is classified at commit time, serialized with the write on a per-slot lock:

- Slot absent → require `new.peer=all` (provisioning).
- Slot present with byte-identical content → require `new.peer=all` OR `change.psk=all`. An identical rewrite changes nothing, so accepting either control preserves retry safety without widening authority: neither control alone gains anything it could not already do.
- Slot present with different content → require `change.psk=all` (rotation).

Races classify at commit under the same lock, so concurrent installs serialize into one of the three cases deterministically. The pinned test is successful-write / lost-response / same-authority retry: a caller holding only `new.peer=all` retries an identical install after the slot appeared and succeeds.

### 5. Explicit secret-transfer boundary: trusted coordinator payloads, forbidden surfaces

The prior draft's "every response omits the PSK" was impossible: issuance MUST carry the PSK to the coordinator (existing `new peer` Response contract), and split operation MUST carry it back as the install argument. The boundary is therefore stated explicitly (option (a)):

- ALLOWED secret-bearing surfaces: coordinator process memory; the issuance Response wire payload (carries exactly one PSK and no other secret); the install operation's explicit secret argument in request memory and structured payloads.
- FORBIDDEN surfaces: stdout/stderr rendering, logs, files other than the destination slot, snippets, diagnostics, configuration, and any persistence written by relay or adapters.
- Split MCP operation exposes the PSK to the MCP client (tool result, then tool argument) BY CONSTRUCTION. Operators choosing split operation explicitly accept MCP-client exposure; the recommended path is single-invocation coordination (CLI link), which keeps the secret in one process's memory. No sealed-transfer or escrow mechanism is introduced: it would be new secret-bearing surface for a threat the explicit boundary already covers.

### 6. Per-step timeout recovery; no cross-relay transaction

Each relay call carries its own timeout, and each unknown state has its own forward recovery — specified PER MODE, because paired mode requires retained mints that one-way mode discards. No automatic compensation anywhere (a compensating drop would race concurrent use):

- One-way registration timeout (alias record commit unknown, mint unneeded): retry `new peer`; success means it was absent, second-claim rejection means it committed — the error disambiguates, then proceed. Existing-record tolerance belongs ONLY to one-way mode.
- Paired registration timeout (committed registration with lost Response leaves no K1/K3 to proceed with): recover by rotating that record to a known PSK and retaining it, or drop-and-restart the pairing. Blind retry can never recover the lost mint.
- Issuance timeout (hash may be committed, PSK response lost): the PSK is unknowable on loss, so retry is useless — recover via rotate-and-install (`change psk` yields a fresh known PSK, then install) or drop-and-relink.
- Install timeout (treated as install-unknown): retry install with the same PSK — safe exactly because install is idempotent under decision 4's identical-content case.

### 7. Install durability is pinned; uncertainty is typed, never silent

Install publication SHALL be temporary-file write plus file fsync, atomic rename, then destination-directory fsync. A directory-sync failure AFTER rename SHALL return a typed durability-uncertain outcome WITHOUT rolling back the file (rename already published; removal would race concurrent readers). The guarantee is bounded: an acknowledged install is durable; an uncertain outcome is reported, never silent, and recovers via idempotent rewrite.

### 8. Confinement: shared no-follow helper, retained dirfds, store included

All state-root-owned credential writes (sink mkdir/chmod/temp/rename for both credential commands, peer-slot install mkdir/open/temp/rename/cleanup for `peers/<alias>.psk`, plus `PrincipalStore` load/persist) go through one traversal helper: every component resolved without following symlinks, symlinked ancestor aborts with `validation_invalid_credential_path` naming the component, and all post-staging operations execute relative to retained directory handles anchored at the trusted state root. Safe alias grammar does not protect a symlinked `peers/` ancestor, so install is enumerated explicitly rather than implied. Store adoption is mandatory with pinned verification: the existing `PrincipalStore` concurrency suite passes unchanged, the fault-injection paths (`.fault-pre-rename`, `.fault-dir-sync`) exercise the same error paths, and the on-disk shape after `persist` is byte-identical to today. `Path` destinations are EXCLUDED from ancestor traversal (pre-existing caller-owned parents outside the trust anchor, routinely containing legitimate symlinks such as macOS `/tmp` → `/private/tmp`); Path keeps its final-target symlink refusal only.

## Risks / Trade-offs

- [Risk] Operators who intentionally symlink state subdirectories now get rejections → Mitigation: fail loudly with `validation_invalid_credential_path` naming the component; bind mounts keep working.
- [Risk] Partial link state (issued-but-uninstalled credential; registered-but-unlinked alias) → Mitigation: per-step recovery ladder (disambiguating retry → rotate+install → drop+relink); no silent auto-compensation racing concurrent use.
- [Risk] PSK lives in coordinator payloads and memory, and split MCP exposes it to the MCP client → Mitigation: explicit boundary (decision 5); forbidden surfaces enforced in relay and adapters; single-invocation CLI recommended; memory-locking out of scope and stated.
- [Risk] Symmetric-naming mismatch in paired mode → Mitigation: declared cross-equality verified before any install; residual config error surfaces at first delivery under existing auth validation.
- [Risk] macOS/Linux portability of no-follow opens (no `openat2` on macOS) → Mitigation: portable `openat` + `O_NOFOLLOW`/`O_DIRECTORY` semantics on both; unit tests on both CI platforms.
- [Risk] Inbound/outbound alias confusion → Mitigation: scenarios pin both inbound records and both resulting slot filenames; install requires the alias referent.

## Migration Plan

- New `link peer` CLI command (one-way and paired modes) and `link` MCP surface; new install operation. Additive.
- New rejections: symlinked ancestors under the state root abort credential and store writes (bind mounts or real directories required).
- Partial-link states are possible and recover forward per decision 6; no revert restores pre-link state automatically — intentional.
- `[[peers]]` entries are added after linking; operators with existing entries link against the running relays and keep their entries (alias records already exist → registration step reports second-claim, coordinator proceeds to issue/install — specified in the one-way scenarios).

## Open Questions

None. Endpoint selection is normative via the `<state-root>/relay.sock` mapping (was left to implementation); transfer boundary, paired algorithm, timeout recovery, durability, and auth classification are all specified above. Exact CLI flag spelling and MCP argument naming follow the semantic parameters pinned in the specs and are left to implementation within those semantics.
