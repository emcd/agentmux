## 1. Implementation

- [ ] 1.1 Define public relay runtime/configuration types with explicit
      configuration root, state root, and Agentmux-owned internal state layout.
      Reject fixed parent-layout assumptions and standalone-only root behavior
      in embedded initialization.
- [ ] 1.2 Define public dispatch handlers for existing relay operations,
      including send, list, look, raww, choose/list decisions, lifecycle, and
      identity introspection.
- [ ] 1.3 Define public principal provisioning and credential APIs over the
      relay identity store, including host-supplied identity descriptors and
      one-time credential return behavior. Include `direct_psk` support for
      host-held in-memory credential material.
- [ ] 1.4 Define a public verified principal context type and require public
      handlers to receive that context rather than trusting request payload
      sender fields.
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
- [ ] 1.10 Define a public transport-neutral delivery executor contract over
      resolved delivery input and typed outcomes. Keep internal delivery task
      fields, worker registry entries, and outcome-completion functions private.
- [ ] 1.11 Add typed delivery lifecycle observation and controlled shutdown to
      the public runtime handle. The runtime must retain worker registration,
      receipt generation, correlation, and shutdown-gate invariants.

## 2. Testing

- [ ] 2.1 Add in-process dispatch tests for send/list/look/raww behavior using
      public handlers and verified principal context.
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
      disposition and removes pending correlation state.
- [ ] 2.8 Add deterministic public-runtime tests using an injected delivery
      executor to drive delivered, failed, timed-out, and shutdown-drop outcomes
      without a live tmux or relay socket pipeline.
- [ ] 2.9 Add public-runtime shutdown-race tests proving dispatch cannot
      resurrect delivery workers after shutdown begins and observers receive the
      relay-authored terminal disposition.

## 3. Validation

- [ ] 3.1 Run `openspec validate embeddable-runtime-api --strict`.
- [ ] 3.2 Run `cargo check --all-targets --all-features`.
- [ ] 3.3 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 3.4 Run `cargo test --all-features`.
