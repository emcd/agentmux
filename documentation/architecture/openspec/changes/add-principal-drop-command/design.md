## Context

Peer ingress `scope` is a literal match, not a policy tier. `scope_permits`
accepts a scope covering either an exact `session@bundle` principal id or a bare
bundle namespace; anything else matches nothing, and an absent scope is
fail-closed. The policy-tier vocabulary (`none`, `self`, `home`, `all`) belongs
to a different concept — session-policy controls — and shares no values with
ingress scope. An operator moving between the two surfaces sees `all` accepted
in both places and reasonably reads it as a wildcard in both.

The receiving side of this was addressed separately: a scope covering no
namespace on the receiving relay is now recorded in that relay's inscriptions
rather than refused. That gives an operator evidence after the fact. It does not
give them a way to fix the principal, and it does not warn at mint time.

The principal store already supports record deletion —
`remove_by_principal_id` is used by both credential-write rollback paths.
Nothing exposes it.

## Goals / Non-Goals

**Goals:**

- Make a mis-scoped or orphaned principal correctable without editing the
  principal store by hand.
- Reach the revocation contract `relay-identity` already specifies from an
  operator-invocable command.
- Warn at mint time when `--scope` is given a policy-tier word.

**Non-Goals:**

- Changing what ingress `scope` means or how `scope_permits` matches. The
  vocabulary collision is a discoverability problem, not a matching bug.
- Deleting credential files from disk.
- Bulk or pattern-based dropping. One principal per invocation.
- Reworking the `Unified Agentmux Command Topology` requirement, which two
  unstarted changes already modify in mutually incompatible ways. `new peer`
  and `change psk` have no `cli-surface` coverage either; `drop peer` matches
  how its siblings are treated today.

## Decisions

### Dropping rather than scope mutation

The ticket offered two shapes: a `--scope` mutation path on `change psk`, or a
drop command. Dropping is the better root fix.

Dropping completes the lifecycle. Mint and rotate exist; delete is the missing
terminal operation, which is why the store has a deletion primitive that no
surface reaches. It also subsumes scope correction — with the record gone,
`new peer` no longer refuses the id, so drop-then-remint corrects *any* wrong
field rather than only `scope`. And it is the only shape that addresses the
orphaned-principal case, where nothing about the record needs correcting because
the record should not exist at all.

Bolting `--scope` onto `change psk` would also erode a deliberate contract:
rotation preserves type, scope, and metadata precisely so that renewing a
credential cannot silently re-authorize a principal. Adding a scope mutation to
that command makes rotation a re-scoping surface.

**Trade-off, stated plainly:** drop-then-remint rotates the PSK. An operator
fixing only a scope must redistribute the new credential to the peer, where a
`change scope` command would have left it alone. This is accepted: fixing a
botched principal by reissuing its credential is defensible hygiene, and the
cost falls on a path that is already a manual two-relay setup. If it proves
annoying in practice, adding `change scope` later is purely additive and
conflicts with nothing here.

### Dropping revokes rather than merely deleting

*Revocation and Expiry Enforcement* already specifies the behavior for an
explicitly revoked principal, and it is the correct behavior here: a principal
whose record is gone must not keep a live authenticated session. Dropping
therefore reuses the same helpers `change psk` uses — `revoke_streams_for_identity`
for the principal's own bound sessions and `notify_trusted_hosts_of_revocation`
for watching trusted hosts.

This makes dropping the first true caller of that contract. Rotation reaches the
same teardown, but as a consequence of the credential changing under a principal
that still exists; dropping is the case the requirement was written for.

Ordering follows `change psk`: mutate and persist the store first, then revoke.
A persist failure must revoke nothing, because the principal still authenticates.

### Self-drop is refused rather than ordered around

Revocation matches on the authenticated identity of a live stream, so dropping
the principal one is authenticated as tears down the connection carrying the
request. The store mutation has already committed at that point, so the operator
receives a transport failure for an operation that succeeded and cannot
distinguish it from one that did not — the worst available outcome for a
destructive command, since the natural response is to retry.

The alternative is to define a success-before-revocation ordering for the
requester's own connection. That requires a guarantee the transport does not
currently offer — that a response is flushed and observed before the eviction
closes the socket — so specifying it would be specifying something the
implementation cannot honor. Refusing self-drop needs no such guarantee: the
handler already holds the requester id, the check is exact, and the rule is
explainable in one sentence. An operator who genuinely wants to drop their own
principal can do so from another authenticated principal, which the `all`-tier
grant requirement already implies they have access to.

The check runs before authorization, and the *other* validation in this command
does not: `validation_unknown_principal` is decidable only by reading the
principal store, so returning it to a caller without a `drop.peer` grant would
disclose whether an arbitrary principal exists. The self-drop check has no
such property — the single identity it can reveal is the caller's own — so it is
safe in front of authorization where the store-dependent check is not. The
operative distinction is whether a validation needs privileged state to decide,
not whether it is a validation.

That distinction could not stay local prose. The Error Object Contract said
flatly that validation failures precede authorization denials, so a Drop Tool
requirement asserting the opposite for one check would have left the spec with
two contradictory rules and no stated precedence. The contract is therefore
amended in this change to define the boundary itself: locally decidable
validations precede authorization, store-backed ones follow it, and a tool's own
requirement classifies its checks.

The amendment is written as a clarification rather than a behavior change.
Shipped `change psk` already authorizes before its unknown-principal lookup, so
the flat rule described neither the intent nor the implementation; naming the
criterion makes existing behavior conformant instead of retroactively
non-conformant. It deliberately does not reclassify any existing tool's checks,
which is why the boundary defers to per-tool requirements rather than
enumerating validations globally.

This bounds the hazard for dropping only. The same shape exists today in
`change psk`, where self-rotation commits a new credential and then tears down
the connection carrying the only copy of the raw PSK — a permanent lockout with
the same hand-edit recovery this change exists to eliminate. That is a defect in
shipped behavior rather than something this change introduces, so it is tracked
separately rather than repaired here.

### Diagnostics travel in the payload, not on stderr

The mint-time warning was first described as stderr output, which does not work
for either caller. The relay is a separate process reached over a socket: its
stderr is neither the CLI client's stderr nor anything an MCP client observes,
and MCP has no stderr channel at all. A warning emitted there would reach the
relay's own log and no operator.

The advisory therefore travels as structured data on the success response —
an optional `diagnostics` array of `code`/`message` pairs — which the MCP tool
preserves in its structured result and the CLI renders to its own stderr. This
keeps one relay-side rule with two faithful surface renderings, rather than a
behavior that silently exists on only one surface.

The array is general rather than a single scope-specific field because the
shape recurs: any advisory the relay wants to attach to a successful
identity-administration response has the same transport problem.

### Credential files are reported, not deleted

`new peer` may have written a PSK to a caller-supplied path or to the relay's
canonical location. Once the store record is gone, that file authenticates
nothing — it is inert, not dangerous. Deleting it is the riskier option: for a
Path destination the relay was handed an arbitrary operator-chosen location, and
for a peer relay the file the operator actually cares about lives on the *other*
relay's disk, which this relay cannot see.

Dropping therefore deletes no files and leaves cleanup to the operator who knows
where the credential was actually distributed.

The reported path is session-only, and omitted for relay, user, and application
principals. The relay owns a canonical credential location for session
principals alone; for a peer relay the credential lives under the *connecting*
relay's state root. A dropping relay that derived a path from its own state root
would name a plausible-looking file that is not the operator's credential — the
same defect the mint-time config snippet already has, where a peer's suggested
storage path is rendered against the local relay's state root rather than the
peer's. Reporting nothing is the honest answer, and `principal_type` is already
in the payload for a caller that wants to explain the omission.

No prose "cleanup hint" field is added for relay principals. A machine payload
that sometimes carries a path and sometimes carries advice about paths is worse
than one that carries a path or nothing.

### A new authorization family, not a reused one

Dropping gets `RelayActionFamily::Drop` and a `drop` control namespace rather
than reusing `change`. Dropping a principal and rotating its credential are
different authorities, and a deployment that grants an automation the ability to
renew credentials should not thereby grant it the ability to delete principals.
Fresh grants fail closed: an existing policy file with no `drop.peer` entry
permits no dropping until an operator adds one.

### Mint-time collision is a warning, not a rejection

`--scope all` cannot be rejected on syntax. A bare bundle namespace is a valid
scope, and `all` is a syntactically valid bundle name — a deployment could have
a bundle called `all`, for which the scope would be correct. Rejection would
refuse a legitimate configuration to protect against a likely mistake.

Existence checking is also wrong here: peer credentials are routinely minted
before the namespace they scope exists, and for cross-relay use the scope may
name a namespace on a relay this one cannot inspect.

What is decidable is the vocabulary collision itself. When `--scope` receives
one of the four policy-tier words, the operator has plausibly confused two
surfaces, and saying so costs nothing and refuses nothing. The relay returns it
as an advisory diagnostic on the success response rather than as a failure — MCP
preserves it in the structured result, the CLI renders it to stderr, and the
command still exits zero. It reports a suspicion, not a fault, which is
consistent with the receiving side recording an unmatched scope rather than
refusing it.

## Risks / Trade-offs

- **Dropping is destructive and has no undo** → The store record is the only
  copy of the credential hash, so dropping invalidates the peer's credential
  permanently. Mitigated by scope: one principal per invocation, no pattern
  matching, and an `all`-tier grant required. Re-minting restores service at the
  cost of credential redistribution, which is the same cost `change psk` already
  imposes.

- **Warning text could train operators to ignore stderr** → Mitigated by
  narrowness: it fires only on four exact values, not on any scope the relay
  merely fails to resolve. A scope that is simply unknown stays silent.

- **A `drop` grant is new configuration surface that no existing policy file
  has** → Failing closed is correct, but an operator hitting
  `authorization_forbidden` on a brand-new command may read it as a bug. The
  error already names the required control, which is the discoverable path.

- **The vocabulary collision remains genuinely confusing** → This change warns
  about it rather than resolving it. Renaming one of the two concepts would be
  the real fix and is a much larger change touching policy configuration; it is
  deliberately not attempted here.

- **Refusing self-drop leaves the analogous `change psk` hazard standing** →
  Dropping is made safe; self-rotation is not, and it is the more damaging of
  the two because the lost response carries the only copy of the new credential.
  Tracked as its own defect rather than widened into this change, which would
  otherwise grow a repair to shipped rotation behavior under a proposal about
  adding a command.
