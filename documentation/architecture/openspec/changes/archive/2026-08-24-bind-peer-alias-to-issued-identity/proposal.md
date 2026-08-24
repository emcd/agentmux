## Why

Every cross-relay message is delivered misattributed. A `Send` from
`coordinator@agentmux` arrives on the peer rendered as
`From: rnd-main@RELAY@RELAY` — the forwarding relay's own identity, doubled —
so a recipient cannot tell who wrote to them and a reply derived from the
envelope targets the wrong principal. Working inter-relay communication is the
release bar, and delivery that succeeds while naming the wrong sender does not
meet it.

The origin identity is not lost: the forwarding relay stamps it as
`on_behalf_of` and it survives intact to the delivered message. What is missing
is the other half of the name. Naming the *relay* a message came from requires
this relay's own name for that peer, and it cannot derive one.

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
- The delivered sender for a cross-relay message becomes
  `<origin>!<peer-alias>`, composed from the forwarded `on_behalf_of` and this
  relay's now-derivable name for the peer that asserted it. The origin is copied
  rather than inspected, so it carries whatever the peer stamped — a bundle
  session, a relay-wide `@GLOBAL` user, or something that names no routable
  recipient at all. It is a reply address for the first two, which are what a
  conforming forwarding relay stamps; for the rest it records provenance and a
  reply to it fails at resolution rather than being routed.
- The identity is composed where the delivered message is built, so the pane
  envelope, the `incoming_message` event and the envelope metadata record all
  name the same sender. Composing at render time would correct the envelope and
  leave the event and the audit record still attributing the message to the
  forwarding relay.
- Cross-relay target resolution accepts a relay-wide `@GLOBAL` origin alongside
  a bundle session, so a sender attributed to a relay-wide user parses back as a
  target. It widens no further: the application and peer relay namespaces stay
  unsupported, and an unqualified principal stays rejected.
- Namespace qualification stops double-qualifying an already-qualified id, which
  is what produces `@RELAY@RELAY` today.

Absent `on_behalf_of` — an unauthenticated origin, which the forwarding relay is
already required not to attribute — the sender remains the peer relay principal,
qualified once. That fallback is existing specified behavior and is preserved.

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
  named by the local part of its authenticated principal. Its sender-attribution
  requirement gains the receiving relay's obligation to compose the delivered
  sender from that name.
- `relay-identity`: *Sender Attribution Schema* reconciles its prohibition on
  interpreting `on_behalf_of` with composing a display identity from it, without
  weakening the authorization or non-resolution prohibitions.
- `pane-envelope`: *Address Identity Format* admits the bang-path in the
  `session:` identity token for a cross-relay sender. The requirement's own
  stated purpose — that a recipient "derive a reply address from the envelope
  alone" — is what a bare `session@namespace` cannot satisfy across relays,
  having no way to name the originating relay.
- `look-and-stream-events`: `sender_session` admits the bang-path for a
  cross-relay sender. That requirement is already violated today, since
  `rnd-main@RELAY@RELAY` is not a valid `session@namespace` either, so this
  chooses which direction restores compliance.
- `relay-routing-layer`: *Cross-Relay Target Classification* accepts a
  relay-wide `@GLOBAL` origin alongside a bundle session, so a rendered
  cross-relay sender parses back, while keeping every existing rejection.

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

Delivered-message construction, the namespace qualification helper, the
cross-relay sender identity stamp, and cross-relay target resolution.

Consumers of an incoming `sender_session` were surveyed: the sites that parse a
session id read the local session's own identity rather than an incoming
envelope's sender, and the one consumer of an incoming sender stores it for
display without parsing. To be re-confirmed during implementation rather than
assumed.

Out of scope: replacing mutual peer registration with one-sided registration,
which would supersede this arrangement rather than extend it — there a relay
issues only to peers it receives from and holds no outbound table, so the
registration requirement above disappears. Also out of scope: per-origin-principal
ingress filtering consuming `on_behalf_of`, and multi-hop attribution chains,
both already deferred by the live requirements.
