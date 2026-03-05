## Context

`tmuxmux` is a local-first relay for LLM coding sessions running inside tmux.
The MVP targets same-host, same-user coordination where MCP tools can inject
human-visible and machine-parseable messages into agent sessions.

The design intentionally avoids extra helper daemons inside each session and
avoids asynchronous ACK protocol layers in the first release.

## Goals / Non-Goals

- Goals:
  - Provide MCP-first message delivery to one session, selected session
    subsets, or all bundle sessions.
  - Keep routing simple with session names as the external addressing model.
  - Reduce agent interruption by waiting for quiescent panes before injection.
  - Keep envelopes strict for machine parsing and readable for humans attached
    to tmux.
- Non-Goals:
  - Cross-host or cross-user transport.
  - Durable queue recovery across `tmuxmux` process restarts.
  - Accept/done protocol acknowledgements.
  - Urgent bypass of quiescence-gated delivery.
  - TUI authoring workflows.

## Decisions

- Decision: session names are the routing primitive.
  - Why: callers can reason about stable identities without tracking pane churn.
  - Consequence: implementation still resolves a concrete pane internally,
    because tmux `send-keys`/`capture-pane` operate on panes.
  - Consequence: pane resolution occurs at delivery time and uses the target
    session's active pane.
  - Consequence: one message can target a selected subset of sessions without
    requiring full broadcast.

- Decision: no relay shim process in target sessions for MVP.
  - Why: reduce startup friction and operational complexity.
  - Consequence: quiescence and injection semantics rely only on tmux commands.

- Decision: strict JSON envelope with pretty-printing.
  - Why: easy machine parsing and direct human readability in attached clients.
  - Consequence: field schema must stay stable and versioned.

- Decision: no asynchronous ACK model in MVP.
  - Why: direct MCP call results can report whether tmux injection succeeded.
  - Consequence: delivery result means "injected to pane input", not
    "semantically processed by target agent."

- Decision: best-effort in-memory queue only.
  - Why: message durability is partially offloaded to agent harness histories,
    and MVP favors simplicity.
  - Consequence: queued messages may be lost if `tmuxmux` exits unexpectedly.

- Decision: tmux socket path is configurable.
  - Why: operators may isolate sessions by socket and can avoid stale default
    sockets.
  - Consequence: every tmux call path must consistently apply the selected
    socket.

- Decision: reconciliation creates sessions directly.
  - Why: `tmux start-server` may not leave a durable running server in
    isolation and is not enough to guarantee target session readiness.
  - Consequence: reconciliation uses `has-session` checks plus `new-session`
    creation flows for readiness.

- Decision: bundle startup uses deterministic bootstrap plus parallel fan-out.
  - Why: this avoids choosing an arbitrary first session and reduces bring-up
    time for large bundles.
  - Consequence: reconciliation creates one deterministic missing session first,
    then creates remaining missing sessions concurrently.

- Decision: reconciliation includes bounded retries with jitter.
  - Why: tmux socket/server readiness races can be transient.
  - Consequence: startup logic retries transient failures before declaring a
    member unavailable.

- Decision: tmuxmux tags owned sessions and performs socket cleanup.
  - Why: ownership metadata enables safe pruning and prevents idle server leaks.
  - Consequence: sessions created by tmuxmux receive an ownership marker and
    dedicated sockets with zero owned sessions are terminated.

- Decision: `exit-empty` remains default for MVP.
  - Why: avoiding persistent idle servers is safer by default.
  - Consequence: tmuxmux relies on direct session creation for readiness rather
    than setting `exit-empty off` globally.

## Quiescence Strategy (MVP)

Before injection into a target session:

1. Resolve the session's active pane.
2. Capture a trailing pane snapshot.
3. Wait for configured `quiet_window_ms`.
4. Capture again and compare snapshots.
5. If unchanged, inject; if changed, repeat until timeout.

If timeout is reached, return a delivery failure for that target.

Default values:

- `quiet_window_ms`: `750`
- `delivery_timeout_ms`: `30000`

Documentation caveat:

- User-facing documentation will warn that continuously changing pane output
  (for example clock-like statusline content) can prevent quiescence detection.

## Risks / Trade-offs

- Snapshot comparison is heuristic; some active workloads may appear stable for
  short windows.
- Waiting for quiescence can delay urgent messages.
- Session-level routing can obscure multi-pane intent when users split panes.
- Parallel startup needs bounded concurrency to avoid excessive burst load.

## Migration Plan

1. Implement MCP operations and tmux adapter behind the new capability.
2. Validate behavior against local same-user tmux sessions.
3. Add optional post-MVP extensions:
   - explicit pane targeting overrides,
   - asynchronous accept/done acknowledgements,
   - durable queue storage.

## Open Questions

- Should session-to-pane resolution remain "active pane" or become configurable
  per bundle member?
