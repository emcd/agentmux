## 1. Implementation

- [ ] 1.1 Define public relay runtime/configuration types with explicit
      configuration root, state root, and Agentmux-owned internal state layout.
      Reject fixed parent-layout assumptions and standalone-only root behavior
      in embedded initialization. Each live handle owns its registry, ledger,
      and shutdown gate per instance so two simultaneous handles are fully
      isolated.
- [ ] 1.2 Define public dispatch handlers for existing relay operations,
      including send, list, look, raww, choose/list decisions, lifecycle, and
      identity introspection. Operation inputs carry an explicit
      operation-bundle parameter; bare-target defaulting follows the
      suffix > parameter > bundle-scoped-home > relay-wide-reject precedence;
      broadcast requires the parameter with no home fallback.
- [ ] 1.3 Define public principal provisioning and credential APIs over the
      relay identity store, including host-supplied identity descriptors and
      one-time credential return behavior. Include `direct_psk` support for
      host-held in-memory credential material.
- [ ] 1.4 Define a public verified principal context type and require public
      handlers to receive that context rather than trusting request payload
      sender fields. The context carries principal-home as an explicit field
      set once at verification time. The type is opaque and non-caller-
      constructible, bound to the issuing runtime/identity-store instance
      with a credential-binding-epoch lifetime (rotation/revocation/deletion
      invalidates; recreation starts a new epoch; shutdown rejects dispatch).
      Name the trusted administrative boundary for provisioning and
      introspection; introspection SHALL NOT mint dispatch authority.
- [ ] 1.5 Refactor the standalone relay binary to construct and call the public
      runtime API as its handler path.
- [ ] 1.6 Refactor the Unix socket protocol layer into a framing adapter over
      public handlers; keep private Hello/request/response frames private.
- [ ] 1.7 Refactor MCP, CLI, and stdio surfaces so relay semantics are provided
      by the same public handlers rather than surface-local logic.
- [ ] 1.8 Add `Content-Type` envelope discrimination with `text/plain`,
      `application/x-agentmux-event+json`, and
      `application/x-agentmux-ext+json` recognition.
- [ ] 1.9 Add bounded `accept_ack` pending-correlation cleanup for content types
      that require accept acknowledgement.
- [ ] 1.10 Define a public typed delivery lifecycle observation contract over
      resolved delivery outcomes. Keep delivery execution, internal delivery
      task fields, worker registry entries, and outcome-completion functions
      runtime-owned and private. No public executor or transport injection
      point.
- [ ] 1.11 Add typed delivery lifecycle observation and controlled shutdown to
      the public runtime handle. The runtime must retain worker registration,
      receipt generation, correlation, and shutdown-gate invariants.

## 2. Testing

- [ ] 2.1 Add in-process dispatch tests for send/list/look/raww behavior using
      public handlers and verified principal context, including bare-target
      defaulting precedence (suffix > parameter > bundle-scoped home) and
      relay-wide rejection without resolvable scope; broadcast without a
      parameter rejected for bundle and relay-wide principals; cross-instance
      context rejection; rotation/revocation invalidating outstanding context
      without resurrection on recreation; introspection output rejected as
      dispatch context.
- [ ] 2.2 Add parity tests proving standalone socket and in-process dispatch
      produce equivalent routing, authorization, response, and attribution
      outcomes for representative operations.
- [ ] 2.3 Add tests proving caller-supplied identity descriptors cannot be used
      as verified principal context.
- [ ] 2.4 Add provisioning tests covering principal creation, one-time raw
      credential return, metadata persistence, and later authentication with a
      direct in-memory credential source.
- [ ] 2.5 Add transport adapter tests proving socket/MCP/CLI/stdio paths call the
      public handler contract and preserve relay-authored errors.
- [ ] 2.6 Add envelope tests covering default `text/plain`, Agentmux event, and
      extension Content-Type discrimination.
- [ ] 2.7 Add ACK timeout tests proving `accept_ack_timeout` records a terminal
      disposition and removes pending correlation state; ack-vs-timeout race
      yields exactly one terminal with the loser diagnosed and discarded;
      submission evidence is three-way (transport_ack proves submission,
      provable transport failure proves non-submission as a distinct
      no-effect terminal, otherwise submission-unknown with no timeout and
      no blind replay); accept admitted before its transport_ack records the
      accepted terminal; `text/plain` lifecycles still terminate at transport
      submission with no accept stage.
- [ ] 2.8 Add deterministic public-runtime tests asserting delivered, failed,
      timed-out, and shutdown-drop outcomes through the public observation
      boundary using existing harness techniques, without a live tmux or relay
      socket pipeline and without any injected delivery implementation.
- [ ] 2.9 Add public-runtime shutdown-race tests proving dispatch cannot
      resurrect delivery workers after shutdown begins and observers receive the
      relay-authored terminal disposition; add two-handle isolation tests
      proving one handle's shutdown, delivery, and provisioned state never
      affect the other handle.

## 3. Validation

- [ ] 3.1 Run `openspec validate embeddable-runtime-api --strict`.
- [ ] 3.2 Run `cargo check --all-targets --all-features`.
- [ ] 3.3 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 3.4 Run `cargo test --all-features`.
