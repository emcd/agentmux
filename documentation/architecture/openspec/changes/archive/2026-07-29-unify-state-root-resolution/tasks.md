## 1. State root resolution

- [x] 1.1 Add `AGENTMUX_STATE_DIRECTORY` as the environment tier for the state
  root, ranked below `--state-directory` and above the XDG and home defaults,
  with a blank value treated as absent like every other tier
- [x] 1.2 Remove the `cfg!(debug_assertions)` branches from `resolve_state_root`
  and `resolve_inscriptions_root`
- [x] 1.3 Confirm the inscriptions root still defaults to
  `<state_root>/inscriptions` and therefore follows the state root without
  separate selection
- [x] 1.4 Remove `repository_root` from `RuntimeRootOverrides`
- [x] 1.5 Normalize the resolved state root to a non-empty absolute path,
  resolving a relative value against the resolving process's working directory,
  and reject an empty `--state-directory` with a structured validation error
- [x] 1.6 Bind and connect Agentmux-owned sockets through their parent directory
  (`/proc/self/fd/<n>/<name>`) rather than by full path, so socket length stops
  scaling with state-root depth. Normalization removes the relative-path escape
  hatch deep hierarchies relied on, and the margin is 11 bytes on the current
  worst case
- [x] 1.7 Settle the tmux socket, which is the longest path at 96 bytes and is
  bound by tmux rather than by Agentmux. The candidate is setting the spawned
  tmux process's working directory to the bundle runtime directory and passing a
  relative `-S tmux.sock` (`src/tmux/pane.rs` currently passes the absolute path
  and sets no working directory). Verify against tmux's use of its server working
  directory for default pane directories before adopting it
- [x] 1.8 Report a socket path-length failure as a structured error naming the
  limit and the offending path, rather than the raw bind errno an operator sees
  today

## 2. Deleting the Git provenance

- [x] 2.1 Delete `repository_checkout_root`, `git_common_directory`,
  `repository_root_from_git_common_directory`, `cargo_manifest_declares_agentmux`,
  and `run_git` **in the same commit as task 1.2**. Removing the resolver without
  the branches leaves the repository root permanently unresolved and silently
  collapses repository-local state onto the XDG default
- [x] 2.2 Remove `debug_repository_state_root` and
  `debug_repository_inscriptions_root`
- [x] 2.3 Remove `--repository-root` and stop threading it through
  `commands/shared.rs` and `commands/host/relay.rs`
- [x] 2.4 Confirm no `git` subprocess remains at startup, and drop the
  `git`-on-`PATH` expectation from prerequisites documentation if stated there

## 3. Propagation

- [x] 3.1 Inject `AGENTMUX_STATE_DIRECTORY` at spawn, from the relay's normalized
  absolute state root, overwriting any value merged from coder, bundle, or member
  configuration. Bundle and session stay load-time upsert-if-absent; do not move
  them
- [x] 3.2 Split the two enumerations `BringUpContext::VARIABLE_NAMES` currently
  conflates. It is documented as "every environment variable this context may
  carry", is held equal to `environment_entries` by test
  (`tests/unit/config/bring_up_context.rs`, `tests/unit/config/coder.rs`), and
  its length is used as the count of load-time stamped entries
  (`tests/unit/config/environment.rs`). Leave it as the load-time stamped set —
  bundle and session — and add a separate enumeration of every agentmux context
  variable a child may inherit, including `AGENTMUX_STATE_DIRECTORY`, for
  consumers that sanitize inherited context rather than stamp it
- [x] 3.3 Point the test-harness sanitizer
  (`tests/integration/support/process.rs`) at the inherited-context enumeration,
  so an inherited `AGENTMUX_STATE_DIRECTORY` is cleared between tests. Leaving it
  on the stamped enumeration would let a developer's own state root leak into
  test processes
- [x] 3.4 Assert no `--state-directory` in the in-repo artifacts that actually
  carry an `agentmux host mcp` command line:
  `.auxiliary/configuration/coders/opencode/settings.jsonc`,
  `.auxiliary/configuration/coders/codex/config.toml`, and the shared
  `.auxiliary/configuration/mcp-servers.json`, which is the generation source for
  the Claude/project surface. `coders/claude/settings.json` is **not** in scope:
  it carries environment, tool permissions, and sandbox settings, and names
  agentmux only as tool identifiers. Enforce it in this repository, which is the
  only place the constraint can be checked mechanically, as a **lint** rather
  than a test: what it asserts is a property of committed artifacts, not
  behavior of the crate, and a working-tree read inside the test suite fires on
  an operator's deliberate local override. As a pre-commit hook it judges staged
  content, and a CI step keeps the regeneration case covered on push. These
  artifacts are Copier-template-generated, so the constraint must also be raised
  against the upstream generator rather than assuming a repo-local check covers
  regeneration; that half is out of this repository and tracked as
  `agentmux:issues/20`, which records the constraint, the affected artifacts and
  the steps, so this task closes on the in-repo enforcement plus that record
  rather than on an unevidenced claim about another repository

## 4. Documentation

- [x] 4.1 Document state root resolution in
  `documentation/usage/operations.md`: the four tiers, that one state root is one
  relay, and that isolating a deployment means naming its state root
- [x] 4.2 Write the cutover note: a deployment previously relying on
  repository-local state must stop its relay, then either name the old roots or
  start fresh on XDG and re-register credentials. Nothing moves on disk either
  way. Name **both** roots — the old state and inscriptions roots are siblings
  (`.auxiliary/state/agentmux` and `.auxiliary/inscriptions/agentmux`), so
  supplying only `--state-directory` relocates new inscriptions under it and
  splits the operator's log history without any indication
- [x] 4.3 State plainly that a source build and an installed build launched with
  the same arguments now share a relay, and how to separate them deliberately
- [x] 4.4 Document the two-relay setup for inter-relay work: each relay names its
  own state root, and a peer's `address` is the other relay's socket path beneath
  that root
- [x] 4.5 Update `src/runtime/README.md` root-resolution section, and remove the
  provisional wording in `src/runtime/paths.rs` describing the deleted mechanism
- [x] 4.6 Sync the `runtime-bootstrap` spec Purpose prose, which still describes
  "XDG state root resolution and its build-profile-gated repository-local
  override". Purpose text is outside the requirement deltas, so the archive will
  not correct it

## 5. Verification

- [x] 5.1 `cargo fmt`, `cargo clippy --all-targets -D warnings`, and the full
  nextest suite
- [x] 5.2 `openspec validate --all --strict`
- [x] 5.3 Prove the state-root tier order, including that `--state-directory`
  outranks `AGENTMUX_STATE_DIRECTORY` and that a blank environment value is
  absent rather than empty
- [x] 5.4 Prove a debug build and a release build resolve identical state and
  inscriptions roots from identical arguments — the assertion the deleted
  branches made impossible
- [x] 5.5 Prove a child stays on the spawning relay: a relay started with an
  explicit `--state-directory` spawns a member whose `agentmux host mcp`
  descendant reaches that relay's socket, not the default root's. Drive it
  through an actual spawn rather than by asserting the variable is set, since the
  defect is that the child resolves elsewhere. Give the member a working
  directory different from the relay's, so a relative or unnormalized root fails
  the test rather than passing by coincidence
- [x] 5.6 Prove the authoritative injection: a member declaring
  `AGENTMUX_STATE_DIRECTORY`, both with a conflicting value and with a blank one,
  still reaches the spawning relay
- [x] 5.7 Prove a deep state root works: bring a relay up and reach it through a
  state root long enough that the full `<state_root>/bundles/<bundle>/tmux.sock`
  path exceeds 107 bytes. Construct the fixture to exceed the limit rather than
  approach it, since a test merely near the boundary passes whether or not the
  fix is present
- [x] 5.8 Rewrite check D of `.auxiliary/scripts/verify-release-binary.sh`, which
  asserts the debug/release divergence this change deletes, to assert identical
  resolution across profiles and explicit isolation via `--state-directory`
