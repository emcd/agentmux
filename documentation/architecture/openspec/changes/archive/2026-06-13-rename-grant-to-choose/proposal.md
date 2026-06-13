# Change: Rename `grant` to `choose` and introduce `gives_choices` transport flag

## Why

The term `grant` implies a binary approve/deny operation. ACP surfaces option
arrays — allow once, allow always, deny once, deny always, and future
workflow-choice menus — making selection from a set the correct mental model.
`grant` also collides with the general English sense of "granting access," which
misleads readers unfamiliar with the ACP permission protocol.

## What Changes

- **Policy capability**: `grant` → `choose`. `choose = "home"` reads as "sessions
  in this bundle may resolve choices presented by ACP."
- **Relay wire requests**: `RelayRequest::PermissionList` → `ChoicesList`;
  `RelayRequest::PermissionResolve` → `ChoicesPick`. UI-side wire message
  `permission.resolve` → `choices.pick`.
- **Relay wire events**: `permission.requested` → `choices.requested`;
  `permission.resolved` → `choices.resolved`; `permission.snapshot` →
  `choices.snapshot`; `permission.list` → `choices.list`.
- **Wire field**: `permission_request_id` → `choice_request_id` throughout all
  payloads.
- **MCP surface**: retire the `grant` meta-tool; add a standalone `choose` verb
  tool for submitting decisions, and a `list.decisions` command on the existing
  `list` meta-tool for polling the pending queue. Both are gated on `choose`
  policy capability. The MCP tool surface stays all-verb: `list`, `help`,
  `look`, `send`, `raww`, `choose`.
- **Transport capability flag**: add `gives_choices: bool` to the registered
  session capability set. `true` for ACP-backed sessions (the transport can
  surface choice requests); `false` for Tmux-backed sessions. Operator/UI
  sessions may resolve choices regardless of their own `gives_choices` value —
  the flag describes choice production, not resolution authority.
- **TUI**: rename all TUI labels, state fields, event handlers, and local structs
  that reference `permission` or `grant` wire names to `choose`/`choices.*`.
  No CLI subcommand is introduced — choices are a workflow for continuously-active
  sessions (TUI); the sporadic CLI call pattern is not appropriate here.
- **Naming convention**: the policy capability is `choose` (a verb: "this
  principal may choose"); the wire/MCP/TUI namespace is `choices.*` (a noun:
  "the set of pending choice records"). They share the same root — the capability
  gates the *act*, the namespace names the *records*.
- **ACP wire protocol**: the ACP protocol's `session/request_permission` request
  type and `permissions` capability are upstream names we do not control and are
  not renamed. The relay translates between the ACP protocol boundary and our
  internal `choices.*` surface at the ACP delivery boundary.
- **Error codes**: `runtime_permission_request_already_resolved` →
  `runtime_choices_request_already_resolved`; `runtime_permission_queue_full` →
  `runtime_choices_queue_full`; `runtime_permission_queue_unavailable` →
  `runtime_choices_queue_unavailable`; `runtime_permission_request_cancelled` →
  `runtime_choices_request_cancelled`; `validation_unknown_permission_request` →
  `validation_unknown_choice_request`.

## Impact

- **Affected specs**: session-relay, mcp-tool-surface, tui-surface,
  relay-routing-layer (capability name in Authorization Stage)
- **Affected code**: `src/relay/contract.rs` (request/event variants, field
  names), `src/relay/handlers/permissions.rs`, `src/relay/authorization/`
  (`choose` capability throughout), `src/relay/context.rs`,
  `src/relay/delivery/permission_state.rs` (internal `Permission*` types),
  `src/mcp/server/handlers/grant.rs` (retire; add `choices_list.rs` and
  `choices_pick.rs`), `src/configuration/types.rs` (`can_give_choices()` method),
  `src/tui/` (all `permission`/`grant` references — ~30 symbols including
  `PendingPermissionEntry`, `pending_permissions`, event-name match arms, and
  wire-field constructions), policy config loading and validation,
  `src/relay/delivery/` (event emission sites), integration and unit tests
- **BREAKING**: wire-level rename; all clients must update simultaneously
