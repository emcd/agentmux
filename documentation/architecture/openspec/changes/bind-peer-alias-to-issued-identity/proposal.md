## Why

A relay holds two records describing one peer, with no key between them.

Inbound, it holds a principal record for the identity **it issued** that peer,
created by `new peer <id>@RELAY`. Outbound, it holds a `[[peers]]` row carrying a
freely chosen local `alias` and a `connect-as` — the identity **the peer issued
it**. Registration records that an identity was issued; it never records to whom.

So when a relay authenticates an inbound peer, it cannot determine its own name
for that peer. Nothing joins the principal it just verified to the row that names
it. A delivered cross-relay envelope therefore cannot say which relay a message
came from, and no amount of rendering work fixes that — the fact is absent from
the data model, not merely unread.

## What Changes

- `[[peers]].alias` is redefined: it is no longer a freely chosen local label but
  **the identity this relay issued that peer**. A relay's name for a peer becomes
  something it already holds, so the inbound join disappears rather than being
  bridged — a relay authenticating `bravo@RELAY` names that peer `bravo`, the
  local part of the principal it just verified.
- Configuration validation SHALL check the invariant rather than assume it:
  `<alias>@RELAY` must exist in the principal store as a relay principal.
- **BREAKING**: every peer named in `[[peers]]` must now also be registered on
  this relay, including one this relay only dials and never receives from. The
  registration is what gives the alias something to be.
- **BREAKING**: any deployment whose `alias` differs from the identity it issued
  must update `[[peers]].alias` and relocate or re-provision the outbound
  credential, whose path is stemmed by the alias.

The alias and `connect-as` stay independent, and the change makes that asymmetry
explicit rather than incidental: `alias` is this relay's local return selector and
the identity it issued; `connect-as` is the identity the peer issued this relay,
authenticating the outbound connection. Nothing keys on `connect-as`, and nothing
may — two peers can issue this relay colliding identities, so it cannot be a key.
Identities a relay *issues* are unique by construction, since the principal store
is keyed by principal id, so the redefined alias is structurally unique rather
than unique by operator discipline.

### What this makes possible, and what it deliberately does not

A relay that only *receives* from a peer already needs no `[[peers]]` entry, only
a registered credential. Such a peer becomes nameable for the first time, because
naming now derives from the principal store rather than the outbound table.

It does not become repliable. Reply routing still requires an outbound row, and a
reply naming a peer with none still fails as an unknown peer. That is the honest
outcome: a relay with no route to a peer cannot reply to it, and inventing a name
that does not resolve would be worse than reporting the absence.

## Capabilities

### New Capabilities

None. This constrains an existing configuration field and names an authority that
already exists.

### Modified Capabilities

- `runtime-bootstrap`: *Outbound Peer Relay Configuration* redefines `alias` as
  the identity this relay issued the peer, and requires configuration validation
  to check it against the principal store.
- `cross-relay-routing`: gains a requirement establishing where a relay's name
  for a peer comes from — the identity it issued — and that an inbound peer is
  named by the local part of its authenticated principal.

## Impact

Relay configuration validation gains a principal-store lookup, which is a new
dependency direction for that stage: configuration validation currently reads
files, and will now consult relay state.

The outbound credential path is stemmed by the alias, so redefining the alias
moves it on disk. This is breaking by necessity rather than by choice: the old
alias and the issued identity have no join, so nothing local can compute the new
value, and no automatic migration is possible for the same reason this change
exists. An absent credential at the new path SHALL fail clearly and name what is
missing. No dual-stem fallback, and no guessing — a relay that silently found a
credential under an old name would reintroduce exactly the ambiguity being
removed.

Out of scope: the cross-relay envelope rendering this unblocks, which is a
follow-on change and becomes nearly mechanical once a peer's name is derivable.
Also out of scope: replacing mutual peer registration with one-sided
registration, which would supersede this arrangement rather than extend it.
