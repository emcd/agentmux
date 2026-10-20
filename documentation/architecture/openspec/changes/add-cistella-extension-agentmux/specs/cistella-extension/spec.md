## ADDED Requirements

### Requirement: Host-root inputs and migration eligibility

The extension SHALL resolve discovery roots from its own process seat ambient (`HOME`, plus `XDG_CONFIG_HOME`/`XDG_STATE_HOME` when set), and the mount-for-mount equivalence promise SHALL apply only to seats whose deployments all use default-layout roots; explicit-root deployments stay on hardcoded mounts pending the eligibility check.

#### Scenario: XDG overrides honored

- **WHEN** the guest ambient sets `XDG_CONFIG_HOME` to a non-default directory holding `agentmux` scopes
- **THEN** discovery scans that directory and not the default

#### Scenario: Explicit-root deployment stays hardcoded

- **WHEN** a seat deployment uses an explicit `AGENTMUX_CONFIGURATION_DIRECTORY` root outside the scanned inputs
- **THEN** the eligibility check fails for that deployment and its hardcoded mounts remain

### Requirement: Hello advertisement and epoch gate

The extension SHALL validate the host `hello` frame (protocol epoch and `hello` op) before answering, advertise its role plus the `mounts`, `permissions`, and `cleanup` contribution capabilities, and refuse silently on epoch or shape mismatch.

#### Scenario: Compatible hello converges

- **WHEN** the host sends a `hello` frame with the supported protocol epoch and `hello` op
- **THEN** the extension answers with its role and contribution capabilities and proceeds to `prepare`

#### Scenario: Epoch mismatch refuses before planning

- **WHEN** the host `hello` carries an unsupported protocol epoch or a non-`hello` op
- **THEN** the extension exits non-zero emitting nothing further and plans no mounts

### Requirement: Session-blind scope discovery

The extension SHALL discover scopes at prepare time by globbing host `~/.config/agentmux*` and `~/.local/state/agentmux*`, treating the prepare `session_id` as opaque attribution only, and SHALL NOT read Cistella profile files or process environment to learn profile state.

#### Scenario: Existing scopes discovered

- **WHEN** the host holds `agentmux`, `agentmux-rnd`, and `agentmux-rnd-qa` config/state directories at prepare time
- **THEN** the extension contributes placements for exactly those scopes and no others

#### Scenario: No scopes present

- **WHEN** no `agentmux*` directories exist under host config/state roots at prepare time
- **THEN** the extension answers an empty contribution set and exits successfully

### Requirement: Symlink rejection at discovery boundary

The extension SHALL match `--deny-scope` entries lexically first (a denied scope is skipped even if itself a symlink, without refusal), refuse prepare naming any remaining symlink candidate (checked with `lstat`, no canonicalization), skip non-directory matches, and enumerate scopes in sorted order. Symlinked scopes are unsupported at incubation: a seat holding any must resolve them or stay on hardcoded mounts.

#### Scenario: Protected-subdirectory alias refuses

- **WHEN** host `~/.config/agentmux-rnd` is a symlink to `~/.config/agentmux/secrets`
- **THEN** prepare refuses non-zero naming the candidate and contributes nothing

#### Scenario: Denied-subdirectory alias refuses

- **WHEN** `--deny-scope=agentmux-foo` is passed and `agentmux-bar` is a symlink to `agentmux-foo/sub`
- **THEN** prepare refuses non-zero naming `agentmux-bar` (its lexical name is not denied; the symlink rule fires)

#### Scenario: Denied scope symlink skipped

- **WHEN** `--deny-scope=agentmux-foo` is passed and `agentmux-foo` is itself a symlink
- **THEN** it is skipped without refusal and prepare succeeds over the remaining scopes

#### Scenario: Non-directory match skipped

- **WHEN** a regular file named `agentmux-foo` exists under a scanned root
- **THEN** it is skipped (not contributed, prepare still succeeds)

#### Scenario: Deterministic enumeration

- **WHEN** prepare runs twice over an unchanged seat
- **THEN** both responses list identical scopes in identical order

### Requirement: Read-only discovery lifecycle

The extension SHALL answer `cleanup=[]` for every pre-existing root, perform no mkdir/chmod/hydration/deletion during prepare or teardown, treat a missing config/state parent as empty discovery for that side, refuse distinctly on any other IO error, and contribute independently present config/state paths without creating missing pairs.

#### Scenario: Teardown leaves trees untouched

- **WHEN** prepare runs and the session tears down over a fixture seat
- **THEN** every discovered tree is byte-identical before and after (content, modes, mtimes), and the prepare response carries empty `cleanup`

#### Scenario: Present config without state contributes singly

- **WHEN** `agentmux-rnd` config exists but its state directory is absent
- **THEN** only the config placement and permission are contributed; no state directory is created

#### Scenario: Non-ENOENT IO error refuses distinctly

- **WHEN** scope enumeration fails with a permission error or mid-walk failure
- **THEN** prepare refuses non-zero with a reason distinct from empty discovery

### Requirement: Mount and permission contributions

The extension SHALL contribute one modeless `mounts[]` placement plus one matching `(target, rights)` `permissions[]` entry per discovered scope, with container targets joined under the configured container home (`<home>/.config/<scope>`, `<home>/.local/state/<scope>`), so no placement is ever orphaned from its permission.

#### Scenario: Contributed plan equals hardcoded table

- **WHEN** a fixture seat holds the scopes from the current `agentmux.toml`
- **THEN** the merged plan contains the same (host-source, container-target, effective-rights) triples mount-for-mount

### Requirement: Closed mode policy

The extension SHALL assign `Read` rights to the exact-`agentmux` config scope and `Full` rights to every other config scope and to all state scopes.

#### Scenario: Production config stays read-only

- **WHEN** the exact `agentmux` config directory is discovered
- **THEN** its contributed permission is `Read` (rendering a read-only binding)

#### Scenario: R&D scopes stay writable

- **WHEN** a non-exact scope such as `agentmux-rnd` config or any state directory is discovered
- **THEN** its contributed permission is `Full` (rendering a writable binding)

### Requirement: Prepare frame validation and output bounds

The extension SHALL bound the declared frame length against its inbound ceiling before allocating, validate prepare epoch/op/request-correlation/session-attribution before any filesystem contact, keep the prepare response within the negotiated `max_frame` (the negotiated operative limit per the Cistella extension protocol — guest-proposed, host-clamped; a large advertisement never overrides it), refusing before any partial frame when it would not fit, emit complete protocol frames on stdout only, and send diagnostics to stderr.

#### Scenario: Oversized frame refuses pre-allocation

- **WHEN** a prepare frame declares a length above the inbound ceiling
- **THEN** the extension refuses non-zero without allocating for the body

#### Scenario: Malformed prepare refuses before discovery

- **WHEN** the prepare frame carries malformed JSON, a wrong op, an unsupported epoch, or no session id
- **THEN** the extension refuses non-zero having touched no host paths

#### Scenario: Bounded fixture fits negotiated output

- **WHEN** 8 scopes with moderate path lengths are discovered with the harness negotiating a 64 KiB operative limit
- **THEN** the prepare response serializes within the maximum and is emitted as one complete frame

#### Scenario: Over-limit response refuses before partial frame

- **WHEN** discovered scopes would serialize past the negotiated `max_frame`
- **THEN** the extension refuses non-zero emitting nothing on stdout (no partial frame)

### Requirement: Static argv contract

The extension SHALL accept `--role=`, `--container-home=`, repeatable `--deny-scope=`, and SHALL refuse prepare when `--container-home` is absent or not absolute.

#### Scenario: Denied scope excluded

- **WHEN** `--deny-scope=agentmux-foo` is passed and that directory exists
- **THEN** no placement or permission is contributed for `agentmux-foo`

#### Scenario: Missing container home fails closed

- **WHEN** `--container-home` is absent or relative
- **THEN** the extension refuses prepare non-zero rather than emitting guessed container targets

### Requirement: Fail-closed prepare

The extension SHALL refuse prepare non-zero (never an undeclared session) on discovery errors, gate failures, or response serialization failures.

#### Scenario: Discovery failure refuses

- **WHEN** scope enumeration fails (e.g. unreadable config root)
- **THEN** the extension exits non-zero with no contributed placements
