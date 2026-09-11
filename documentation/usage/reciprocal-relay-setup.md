# Reciprocal Inter-Relay Setup

This guide links two relays on one host so each can deliver to the
other's sessions. It is the end-to-end procedure the
[maintainer configuration guide](maintainer-configuration-guide.md)
`[[peers]]` reference and the [relay README](../../src/relay/README.md)
Cross-Relay sections assume: same-host state roots and sockets,
reciprocal aliases, safe credential provisioning with `link peer`, both
peer entries, restart, and bidirectional verification. No PSK is ever
printed, logged, or copied by hand.

The worked example links relay **alpha** and relay **bravo**. Substitute
your own roots, aliases, and scopes throughout.

## 0. Identify each relay's roots and socket

Every command below names explicit roots, so nothing resolves through
XDG defaults. A relay's socket is `<state-root>/relay.sock`.

| Relay | Configuration root | State root | Socket |
| --- | --- | --- | --- |
| alpha | `/srv/agentmux/alpha-config` | `/srv/agentmux/alpha-state` | `/srv/agentmux/alpha-state/relay.sock` |
| bravo | `/srv/agentmux/bravo-config` | `/srv/agentmux/bravo-state` | `/srv/agentmux/bravo-state/relay.sock` |

Both relays must be running before linking (the coordinator issues
requests against both sockets), but neither needs the other's
`[[peers]]` entry yet — entries are added in step 3, after the
credentials exist. The operator identity used below must hold
`new.peer=all` (and `change.psk=all` for rotations or upgrades) on
**both** relays; if the two relays use different operator sessions, see
the per-side selectors in step 2.

## 1. Choose reciprocal names

Each relay names the peer with a local `alias` and presents a
`connect-as` identity the peer issued it. `alias` and `connect-as` on
one entry name opposite directions and normally differ; across the two
entries they cross-match — each side presents the name the peer gave it:

| Relay | `alias` (this relay's name for the peer) | `connect-as` (name the peer gave this relay) |
| --- | --- | --- |
| alpha | `bravo` | `alpha` |
| bravo | `alpha` | `bravo` |

So alpha presents `alpha@RELAY` to bravo, and bravo presents
`bravo@RELAY` to alpha. Keep this table: the paired link verifies the
cross-match before minting anything (a mismatch fails the link with a
symmetric-naming error, before any credential exists), and the entries
in step 3 must repeat it exactly. A mismatch between the entries and
the table surfaces later as an authentication failure at first
delivery — see [Common errors](#7-common-errors).

Scopes are the inbound grants each side confers: `--alias-scope` is what
the peer may reach on the destination relay, recorded on the alias
principal. Omit a scope for no rights (the peer authenticates but can
reach nothing); pass `--scope '*'` or a comma-separated namespace set
to grant ingress. The full grammar (`'*'`, namespace sets, no-rights
absence, and the `RELAY`/`EXTERNAL` exclusions) lives in
[authorization.md](authorization.md); adjust a granted scope afterwards
with `change scope <alias>@RELAY --scope` (gated on
`change.scope=all`), which replaces it without rotating the PSK.

## 2. Provision both directions with `link peer`

Run the paired flow once. It registers both alias principals (two
mints, both retained), verifies the declared cross-equality from the
table above, and cross-installs the raw credentials — two installs,
zero drops — without rendering either PSK:

```console
$ agentmux link peer \
    --issuer-state-directory /srv/agentmux/alpha-state \
    --destination-state-directory /srv/agentmux/bravo-state \
    --issuer-configuration-directory /srv/agentmux/alpha-config \
    --destination-configuration-directory /srv/agentmux/bravo-config \
    --alias alpha --peer-alias bravo \
    --connect-as bravo --peer-connect-as alpha \
    --paired \
    --alias-scope 'team-one' --peer-scope 'team-two'
link peer: registered bravo@RELAY and alpha@RELAY
link peer: installed /srv/agentmux/bravo-state/peers/alpha.psk and /srv/agentmux/alpha-state/peers/bravo.psk
```

Which side is "issuer" vs "destination" does not matter for paired
mode — the flow is symmetric. If the two relays use different operator
sessions, add `--issuer-as-session NAME` and
`--destination-as-session NAME` (with `--issuer-bundle` /
`--destination-bundle` as needed); they fall back to the shared
`--bundle` / `--as-session` when absent.

One direction only, or an upgrade of an existing one-way link, uses the
same command without `--paired`: one-way registers, issues, and
installs; `--upgrade` rotates the inbound identity and installs with no
registration step. See `agentmux link peer --help` for the full flag
list.

### Credential direction and file ownership

After the paired run above, the secrets rest as follows. Raw PSKs live
only in the two slot files (mode `0600`, parent directories `0700`,
owned by the relay user); each store holds only a hash:

| Secret | Raw file | Matching store record |
| --- | --- | --- |
| K1 | `/srv/agentmux/bravo-state/peers/alpha.psk` (bravo presents `bravo@RELAY` to alpha) | `bravo@RELAY` on **alpha** (hash only) |
| K2 | `/srv/agentmux/alpha-state/peers/bravo.psk` (alpha presents `alpha@RELAY` to bravo) | `alpha@RELAY` on **bravo** (hash only) |

Never copy a raw PSK by hand and never print one: `link peer` is the
only writer these files need. The relay refuses symlinked ancestors
beneath the state root when writing them.

## 3. Add both `[[peers]]` entries

Append to each relay's `relay.toml`, repeating the table from step 1
exactly (`address` is the peer's socket from step 0):

```toml
# /srv/agentmux/alpha-config/relay.toml
[[peers]]
alias = "bravo"
address = "/srv/agentmux/bravo-state/relay.sock"
connect-as = "alpha"
```

```toml
# /srv/agentmux/bravo-config/relay.toml
[[peers]]
alias = "alpha"
address = "/srv/agentmux/alpha-state/relay.sock"
connect-as = "bravo"
```

Raw PSKs never appear here — only the three fields above. Register
every peer listed, including one a relay only dials: startup and
`check configuration` reject an alias with no `<alias>@RELAY`
registration, because a missing record is indistinguishable from a
typo. The paired link in step 2 already created both registrations.

## 4. Validate with explicit roots

Validate each side against its own roots before restarting, so a
shadowing layer or typo fails here and not at startup:

```console
$ agentmux check configuration \
    --configuration-directory /srv/agentmux/alpha-config \
    --state-directory /srv/agentmux/alpha-state
$ agentmux check configuration \
    --configuration-directory /srv/agentmux/bravo-config \
    --state-directory /srv/agentmux/bravo-state
```

Both runs must exit zero. The source report names the physical file
supplying each artifact — confirm the `relay.toml` you edited is the
one in effect (see [operations.md](operations.md)).

## 5. Restart both relays

`[[peers]]` entries take effect at startup: restart each relay (for
example `systemctl --user restart agentmux-relay.service`, or restart
the `host relay` processes). Startup re-validates every entry's alias
registration and fails loudly naming the offending alias. Outbound
peer connections are lazy — a restart never dials, so an unreachable
peer cannot block or destabilize startup.

## 6. Verify delivery in both directions

Send across the bang-path in each direction. From a session on alpha,
address a session on bravo through alpha's alias for bravo, and mirror
it from bravo:

```console
$ agentmux send --target 'sess@bundle!bravo' --message 'alpha to bravo' \
    --configuration-directory /srv/agentmux/alpha-config \
    --state-directory /srv/agentmux/alpha-state
$ agentmux send --target 'sess@bundle!alpha' --message 'bravo to alpha' \
    --configuration-directory /srv/agentmux/bravo-config \
    --state-directory /srv/agentmux/bravo-state
```

Replace `sess@bundle` with real session identities on the far relay.
The MCP `list` tool with `command="relays"` enumerates the configured
outbound aliases without dialing. A missing or unreadable peer
credential fails only the affected delivery with a typed outcome naming
the path — never startup.

## 7. Common errors

| Symptom | Cause | Fix |
| --- | --- | --- |
| `link peer` reports `link_issuance_unknown` / `link_registration_unknown` | A call may or may not have committed; the PSK is unknowable | Recover explicitly: `link peer --upgrade` (rotate and install) or drop-and-relink. Never blind-retry issuance. |
| `paired link requires symmetric naming` | Declared `--connect-as` / `--peer-connect-as` do not equal the opposite alias | Fix the flags to the step-1 table; nothing was minted. |
| `validation_principal_exists` on registration | Record already registered | One-way registration tolerates this and proceeds; paired mode reports unknown (rotate explicitly instead). |
| `validation_unknown_principal` on install | Alias referent missing | Register `<alias>@RELAY` first (the paired/one-way register step does this). |
| `authorization_forbidden` | Operator lacks `new.peer=all` / `change.psk=all` on that relay | Grant the relay-wide control to the operator policy on both relays. |
| Startup / `check configuration` names an alias | `<alias>@RELAY` not registered | Register it, or fix the entry's alias. |
| First delivery fails authentication | Entry `connect-as` does not equal the opposite alias | Correct the entries to the step-1 table; both relays must agree. |
| `relay_unavailable` from `link` / `send` | Relay not running at the socket, or wrong `--state-directory` | Start the relay; confirm the socket path matches step 0. |
| `validation_invalid_credential_path` | Symlinked ancestor beneath the state root | Replace symlinks with real directories or bind mounts. |
| Deliveries fail naming the credential path | Slot file absent or unreadable | Re-run the link flow; check `0600` ownership by the relay user. |
