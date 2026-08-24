## 1. Confirm what keys on the alias

- [ ] 1.1 Enumerate every use of a `[[peers]]` alias — the peer endpoint and
  session maps, the credential path, and bang-path target resolution — and
  confirm each still selects correctly when the alias is the issued identity.
- [ ] 1.2 Confirm nothing keys on `connect-as`. A lookup keyed on it is a defect
  independent of this change, because peers may issue colliding values; record
  it rather than absorbing it here.

## 2. Validate the invariant at configuration load

- [ ] 2.1 Verify each `[[peers]]` entry's `<alias>@RELAY` exists in the principal
  store as a relay principal, failing with a structured validation error that
  names the offending alias and says whether it was absent or of the wrong type.
- [ ] 2.2 Surface the same finding through `agentmux check configuration`, so a
  deployment can be judged without starting a relay.
- [ ] 2.3 Do **not** compare the outbound credential against the store record's
  credential hash. The two are issued in opposite directions; a correctly
  configured deployment would fail such a check.
- [ ] 2.4 Cover an alias that is absent from the store, and an alias present with
  a non-relay principal type, as separate assertions naming which case failed.
- [ ] 2.5 Cover that an entry whose `alias` and `connect-as` differ is accepted
  when the alias names an issued identity. This is the case a symmetric fixture
  cannot reach, and the one the invariant exists for.
- [ ] 2.6 Cover that a peer this relay only dials is rejected when unregistered.
  This is the case the conditional form of the rule would have excused, so it is
  the assertion that distinguishes a real invariant from a typo check.
- [ ] 2.7 Teeth-check 2.4, 2.5 and 2.6 individually by weakening the validation
  and confirming each fails on its own — not through a loop, which stops at its
  first failure and proves nothing about later cases. For 2.6 specifically,
  weaken it the way the conditional rule would (skip the check when no store
  record exists) and confirm that alone lets the case through.

## 3. Name an inbound peer from its authenticated principal

- [ ] 3.1 Derive a peer's name from the bare relay id of the authenticated
  principal on that connection, consulting no `[[peers]]` entry.
- [ ] 3.2 Cover that a peer with no `[[peers]]` entry is still named.
- [ ] 3.3 Cover that naming such a peer does not make it routable: a cross-relay
  target naming it still fails as an unknown peer at delivery.
- [ ] 3.4 Teeth-check 3.2 by making naming consult the outbound table, and
  confirm the receive-only case fails while the ordinary case still passes. That
  asymmetry is the point of the test.

## 4. Fail clearly on a missing credential

- [ ] 4.1 Confirm the outbound credential is read only from the alias-stemmed
  path, and that an absent file fails the affected delivery with a typed outcome
  naming the path.
- [ ] 4.2 Confirm no fallback path is consulted. A relay that finds a credential
  under a name the configuration no longer claims reintroduces exactly the
  ambiguity this change removes.

## 5. Documentation

- [ ] 5.1 Update the relay subsystem README where it describes peer aliasing and
  the peer credential layout, in the same change as the behavior.
- [ ] 5.2 Update the operator-facing peer setup documentation: `[[peers]].alias`
  is now the identity this relay issued the peer, and an existing deployment must
  update the alias and relocate or re-provision the credential. State that no
  automatic migration is possible and why, so the breakage reads as designed
  rather than as an oversight.
- [ ] 5.3 Sweep the usage documentation for prose describing the alias as a
  freely chosen local label, and correct what this change invalidates.
- [ ] 5.4 Run the full suite under both the default and `pty` feature sets, plus
  clippy and format checks.
