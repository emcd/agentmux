# Change: Lock ACP look behavior for MVP as unsupported transport

## Why

`look` currently reflects tmux pane snapshot semantics. ACP transport does not
provide an equivalent snapshot primitive in the current runtime contract.
Without an explicit lock, surfaces may diverge on ACP `look` behavior.

## What Changes

- Lock ACP-target `look` behavior in MVP as explicit unsupported transport.
- Define one stable error code for ACP look rejection.
- Require MCP and CLI adapters to propagate the same relay-authored rejection
  semantics without divergence.
- Preserve existing tmux `look` behavior unchanged.

## Non-Goals

- Implementing synthesized ACP look snapshots.
- Changing `look` request parameters.
- Introducing cross-bundle look support.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Expected follow-up implementation touchpoints:
  - relay look target transport check
  - MCP look passthrough tests
  - CLI look error rendering tests
