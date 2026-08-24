## 1. Confirm what keys on the alias

- [x] 1.1 Enumerate every use of a `[[peers]]` alias — the peer endpoint and
  session maps, the credential path, and bang-path target resolution — and
  confirm each still selects correctly when the alias is the issued identity.
- [x] 1.2 Confirm nothing keys on `connect-as`. A lookup keyed on it is a defect
  independent of this change, because peers may issue colliding values; record
  it rather than absorbing it here.

## 2. Validate the invariant at configuration load

- [x] 2.1 Verify each `[[peers]]` entry's `<alias>@RELAY` exists in the principal
  store as a relay principal, failing with a structured validation error that
  names the offending alias and says whether it was absent or of the wrong type.
- [x] 2.2 Surface the same finding through `agentmux check configuration`, so a
  deployment can be judged without starting a relay.
- [x] 2.3 Do **not** compare the outbound credential against the store record's
  credential hash. The two are issued in opposite directions; a correctly
  configured deployment would fail such a check.
- [x] 2.4 Cover an alias that is absent from the store, and an alias present with
  a non-relay principal type, as separate assertions naming which case failed.
- [x] 2.5 Cover that an entry whose `alias` and `connect-as` differ is accepted
  when the alias names an issued identity. This is the case a symmetric fixture
  cannot reach, and the one the invariant exists for.
- [x] 2.6 Cover that a peer this relay only dials is rejected when unregistered.
  This is the case the conditional form of the rule would have excused, so it is
  the assertion that distinguishes a real invariant from a typo check.
- [x] 2.7 Teeth-check 2.4, 2.5 and 2.6 individually by weakening the validation
  and confirming each fails on its own — not through a loop, which stops at its
  first failure and proves nothing about later cases. For 2.6 specifically,
  weaken it the way the conditional rule would (skip the check when no store
  record exists) and confirm that alone lets the case through.

## 3. Name an inbound peer from its authenticated principal

- [x] 3.1 Derive a peer's name from the bare relay id of the authenticated
  principal on that connection, consulting no `[[peers]]` entry.
- [x] 3.2 Cover that a peer with no `[[peers]]` entry is still named.
- [x] 3.3 Cover that naming such a peer does not make it routable: a cross-relay
  target naming it still fails as an unknown peer at delivery.
- [x] 3.4 Teeth-check 3.2 by making naming consult the outbound table, and
  confirm the receive-only case fails while the ordinary case still passes. That
  asymmetry is the point of the test.

## 4. Fail clearly on a missing credential

- [x] 4.1 Confirm the outbound credential is read only from the alias-stemmed
  path, and that an absent file fails the affected delivery with a typed outcome
  naming the path.
- [x] 4.2 Confirm no fallback path is consulted. A relay that finds a credential
  under a name the configuration no longer claims reintroduces exactly the
  ambiguity this change removes.

## 5. Accept relay-wide origins in cross-relay target resolution

- [x] 5.1 Widen cross-relay target resolution to accept a relay-wide `@GLOBAL`
  principal alongside a bundle-qualified session, and nothing further. The
  application and peer relay namespaces stay rejected as unsupported, an
  unqualified principal stays rejected as unqualified, and an empty or
  separator-bearing relay id stays rejected. The set to admit is what a
  conforming forwarding relay can honestly attribute, not every string a peer
  might assert.
- [x] 5.2 Cover a relay-wide `@GLOBAL` origin resolving, and cover that the
  existing rejections still reject — the application and peer relay namespaces as
  unsupported, an unqualified principal as unqualified. Assert each separately
  with the input named, since widening one arm is the change most likely to
  loosen the others by accident.
- [x] 5.3 Teeth-check 5.2 by restoring the session-only restriction and
  confirming only the relay-wide case fails.

## 6. Compose the delivered cross-relay sender

- [x] 6.1 Stop double-qualifying an already-qualified principal id in the
  namespace qualification helper, so the fallback attribution carries its suffix
  once.
- [x] 6.2 At the delivered-message construction site, compose the sender as
  `<on_behalf_of>!<peer-name>` when `on_behalf_of` is present, taking the peer
  name from the authenticated inbound principal per task 3.
- [x] 6.3 Fall back to the peer relay principal, qualified once, when
  `on_behalf_of` is absent, and carry no synthesized origin.
- [x] 6.4 Reuse the existing bang-path grammar rather than formatting a second
  spelling of it.

## 7. Prove the three surfaces agree, and that a reply resolves

- [x] 7.1 Cover that the pane-envelope `From` and the bare accessor the
  `incoming_message` event and the envelope metadata record both read carry the
  composed identity. The two machine consumers take the same field, so the
  assertion is on that field rather than on each consumer separately; say so
  rather than implying three independent observations.
- [x] 7.2 Teeth-check 7.1 by composing at render time instead of at
  construction, and confirm the event and metadata assertions fail while the
  pane assertion passes. That regression is invisible to a pane-only test.
- [x] 7.3 Feed the rendered identity back through cross-relay target resolution
  and confirm it resolves to the origin and the peer name. Cover a bundle-session
  origin and a relay-wide one.
- [x] 7.4 Teeth-check the separator by altering it in composition and confirming
  the delivered-envelope assertions fail. The resolution test cannot see that
  change, since it builds its own target — which is why the binding is the two
  tests together and neither alone.
- [x] 7.5 Cover that a sender naming a peer with no outbound entry still renders,
  and that a reply to it fails as an unknown peer rather than resolving
  elsewhere.
- [x] 7.6 Cover that local (non-ingress) delivery is unchanged: the sender is the
  bare canonical `session@namespace` id.
- [x] 7.7 Cover a peer asserting an `on_behalf_of` that is not a routable
  principal id — an unqualified string, and one in the application namespace —
  and confirm each is composed and displayed unaltered, with the delivery
  accepted. Assert the two separately with the asserted value named.
- [x] 7.8 Cover that a reply to each of those fails at target resolution with a
  structured validation error and forwards nothing. These are the cases where an
  unconditional reply-derivability promise would have been false, so they are
  what holds the conditional one honest.
- [x] 7.9 Teeth-check 7.7 by making composition inspect the origin and fall back
  to the peer principal when it does not parse, and confirm both cases fail. That
  fallback is the plausible-looking repair the no-interpretation rule forbids.

## 8. Confirm the prohibitions still hold

- [x] 8.1 Cover that an ingress request whose target lies outside the peer relay
  principal's scope is still refused, and that the composed identity plays no
  part in that decision.
- [x] 8.2 Confirm no code path resolves either segment of the composed identity
  against the local principal store.

## 9. Documentation

- [x] 9.1 Update the relay subsystem README where it describes peer aliasing and
  the peer credential layout, in the same change as the behavior.
- [x] 9.2 Update the operator-facing peer setup documentation: `[[peers]].alias`
  is now the identity this relay issued the peer, and an existing deployment must
  update the alias and relocate or re-provision the credential. State that no
  automatic migration is possible and why, so the breakage reads as designed
  rather than as an oversight.
- [x] 9.3 Sweep the usage documentation for prose describing the alias as a
  freely chosen local label, and correct what this change invalidates.
- [x] 9.4 Run the full suite under both the default and `pty` feature sets, plus
  clippy and format checks.
