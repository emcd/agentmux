## 1. Protocol Contract

- [x] 1.1 Define full-duplex relay stream framing model.
- [x] 1.2 Define required `hello` registration payload and validation.
- [x] 1.3 Define identity replacement semantics for reconnecting clients.
- [x] 1.4 Define endpoint class routing behavior (`agent` vs `ui`).
- [x] 1.5 Define relay push event payload contract for inbound message + delivery outcomes.

## 2. Routing and Queue Semantics

- [x] 2.1 Lock static recipient routability independent of recipient hello state.
- [x] 2.2 Lock disconnected UI recipient queue behavior using existing relay queue machinery.
- [x] 2.3 Lock same-bundle MVP scope and cross-bundle rejection semantics.

## 3. Runtime Integration

- [x] 3.1 Define MCP client transport expectations for persistent agent-class streams.
- [x] 3.2 Define TUI client transport expectations for persistent ui-class streams.
- [x] 3.3 Define fail-fast behavior for malformed/missing hello and stream protocol violations.

## 4. Validation

- [x] 4.1 Run `openspec validate add-relay-stream-hello-transport-mvp --strict`.
