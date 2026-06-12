## 1. Relay core (contract + handler)

- [ ] 1.1 Rename request and response variants in `src/relay/contract.rs`:
      - `RelayRequest::PermissionList` → `ChoicesList`
      - `RelayRequest::PermissionResolve` → `ChoicesPick`
      - `RelayResponse::PermissionList` → `ChoicesList`
      - `RelayResponse::PermissionDecision` → `ChoicesPick`
      - Relay-side contract struct `PendingPermissionEntry` (:409) → `PendingChoiceEntry`
      Rename `permission_request_id` → `choice_request_id` in all payload structs.
      Remove the reserved `ui_session_id: Option<String>` field from
      `PermissionResolve` (:255) — unused and actively rejected. Also remove
      the now-dead rejection check at `src/relay/handlers/permissions.rs:163-167`
      (`ui_session_id.is_some()` guard). The `ui_session_id: None` literals in
      `grant.rs:170` and `src/tui/state/compose/permissions.rs:143` are handled
      by tasks 3.1 and 5.1 respectively.
- [ ] 1.2 Rename relay wire events in emission sites:
      - `permission.requested` → `choices.requested`
      - `permission.resolved` → `choices.resolved`
      - `permission.snapshot` → `choices.snapshot`
      - `permission.list` → `choices.list`
      - UI-side `permission.resolve` → `choices.pick`
      Rename the snapshot payload key `permission_request_ids` →
      `choice_request_ids` in the snapshot emission site.
      Rename relay inscription names:
      - `relay.permission.requested` → `relay.choices.requested`
      - `relay.permission.resolved` → `relay.choices.resolved`
      - `relay.acp.respawn.permission_invalidated` →
        `relay.acp.respawn.choice_invalidated`
- [ ] 1.3 Rename error codes — update all match arms and string literals:
      - `runtime_permission_request_already_resolved` →
        `runtime_choices_request_already_resolved`
      - `runtime_permission_queue_*` → `runtime_choices_queue_*`
      - `runtime_permission_request_cancelled` →
        `runtime_choices_request_cancelled`
      - `runtime_permission_request_invalidated_by_respawn` →
        `runtime_choices_request_invalidated_by_respawn`
      - `validation_unknown_permission_request` →
        `validation_unknown_choice_request` (check both relay and TUI sides)
- [ ] 1.4 Rename `grant` capability references in
      `src/relay/authorization/` (loading, checks, policy structs) to `choose`.
      Rename the `grant` field in policy TOML structs and default values.
      Rename `grant_authorized_ui_sessions` in
      `src/relay/authorization/resolution.rs` to `choose_authorized_ui_sessions`.
- [ ] 1.5 Rename internal relay types and helpers: every `Permission*` /
      `permission_*` symbol under `src/relay/` renames to the `Choice*` /
      `choice_*` / `choices_*` equivalent. `src/acp/` retains the `Permission`
      vocabulary at the ACP protocol boundary and is not renamed.
      Run `rg "\bPermission\b|\bpermission_\b|\bpermission\b" src/relay/
      --type rust` to enumerate remaining hits after initial pass.
- [ ] 1.6 Rename TOML config section `[relay.permission]` →  `[relay.choices]`
      (field `max-pending` is unchanged). Update loading code
      (`src/relay/authorization/loading.rs`, `RawRelayPermissionSection` :57 →
      `RawRelayChoicesSection`; `load_permission_max_pending` :266 →
      `load_choices_max_pending`) and any default-value references.

## 2. Transport capability flag

- [ ] 2.1 Add `can_give_choices() -> bool` as a derived method on `SessionType`
      in `src/configuration/types.rs`, alongside `can_be_looked`, `can_be_written`,
      `can_stream_output`. `SessionType` has four variants; match all four
      explicitly (no catch-all). Returns `true` only for `Acp`; `Tmux`, `Ui`,
      and `Pubsub` return `false`.
- [ ] 2.2 `can_give_choices()` has no enforcement site in the
      `choices.list` / `choices.pick` request paths — those address the choice
      record queue, not the producing session's transport. The method exists to
      anchor the capability table in the session-relay spec and to support the
      `decouple-transport-layer` proposal. Exercise it in unit tests only;
      no handler-level gate is added here. (Extending `ListedSession` with a
      `capabilities` field is deferred to a follow-on proposal.)

## 3. MCP surface

- [ ] 3.1 Retire `src/mcp/server/handlers/grant.rs`. Add a new `decisions`
      command to the `list` meta-tool handler (maps to `ChoicesList` relay
      request); add a standalone `choose` tool handler (maps to `ChoicesPick`
      relay request). The registered tool names are `list` (existing) and
      `choose` (new).
- [ ] 3.2 Gate `list decisions` and `choose` on `choose` policy capability
      (same authz as current `grant`). `gives_choices` on the caller's
      transport is not required — any session with sufficient `choose` scope
      may resolve choices.
- [ ] 3.3 Update MCP passthrough error codes to the renamed set from task 1.3.

## 4. CLI surface

No CLI subcommand is introduced by this proposal. Choices are a workflow for
continuously-active sessions (TUI); the sporadic CLI call pattern is not
appropriate here. No work in this task group.

## 5. TUI surface

- [ ] 5.1 Rename all `permission` / `grant` references in `src/tui/`. Key
      symbols to rename:
      - State structs: `PendingPermissionEntry` → `PendingChoiceEntry`,
        `PendingPermissionOption` → `PendingChoiceOption`
        (`src/tui/state/mod.rs:87,98`; note the relay-side struct
        `PendingChoiceEntry` in `src/relay/contract.rs:409` will share the
        same name — different modules, no conflict)
      - State fields: `pending_permissions` → `pending_choices`,
        `pending_permissions_state` → `pending_choices_state`,
        `look_permission_request_index` → `look_choice_request_index`,
        `look_permission_option_index` → `look_choice_option_index`
        (`src/tui/state/mod.rs`)
      - Methods in `src/tui/state/compose/permissions.rs`:
        `ensure_pending_permission_selection` (:70),
        `selected_look_permission` (:93),
        `selected_look_permission_option` (:108),
        `submit_permission_decision` (:121),
        `resolve_selected_look_permission_selected`,
        `resolve_selected_look_permission_cancelled`,
        `move_look_permission_request_selection`,
        `move_look_permission_option_selection`,
        `look_pending_permissions`,
        `upsert_pending_permission`,
        `parse_permission_options`
      - Methods in `src/tui/state/history.rs`:
        `apply_permission_snapshot`, `remove_pending_permission` (:515)
      - Harness method: `inject_pending_permission` (`src/tui/workbench.rs:175`)
      - Free functions: `interaction_permission_active`,
        `render_look_permission_section`, `render_look_permission_lines`,
        `interaction_permission_pane_height`
        (`src/tui/{input,render/interaction}.rs`)
      - Label in `src/tui/render/overlays/help.rs:60`
        ("Permission (write empty...)")
      - Error code: `validation_unknown_permission_request` →
        `validation_unknown_choice_request`
        (`src/tui/state/compose/permissions.rs:40,63`;
        `src/tui/state/mod.rs:550,558`)
      - Event names: `permission.snapshot` → `choices.snapshot`,
        `permission.requested` → `choices.requested`,
        `permission.resolved` → `choices.resolved`
        (`src/tui/state/{history,mod}.rs`,
        `src/tui/state/compose/permissions.rs`)
      - Wire action: `permission.resolve` → `choices.pick`
        (`src/tui/state/compose/permissions.rs`)
      - Wire field: `permission_request_id` → `choice_request_id`;
        snapshot key `permission_request_ids` → `choice_request_ids`
        (`src/tui/state/{history,mod}.rs`,
        `src/tui/render/{interaction,overlays/events}.rs`,
        `src/tui/workbench.rs`)
      - File rename: `src/tui/state/compose/permissions.rs` →
        `src/tui/state/compose/choices.rs`; update `mod.rs` declaration and
        all `use` paths accordingly.
- [ ] 5.2 Update documentation in the same batch:
      - `src/tui/README.md` — module map (references `permissions.rs` and its
        functions), behavior section, `permission.resolve` reference (:187)
      - `documentation/usage/tui.md` (:34, :69–77, :129, :166–171)

## 6. Tests and proof of absence

- [ ] 6.1 Update all unit and integration tests that reference renamed wire
      names, error codes, policy field names, or MCP tool names.
- [ ] 6.2 Proof of absence — before reporting done, run:
      ```
      rg -i grant src/ data/ --type rust
      rg -i grant data/ --glob "*.toml"
      rg -i permission src/relay/ src/tui/ src/mcp/ --type rust
      rg -i grant openspec/specs/
      ```
      `src/acp/` hits referencing `Permission`/`permission` are expected
      (ACP protocol boundary vocabulary — do not rename). All other hits must
      be legitimate (comments for historical context or unrelated English usage).
      Report output with completion message.

## 7. Coordination

- [ ] 7.1 After landing, amend `decouple-transport-layer/tasks.md` to note
      that `can_give_choices()` exists as a derived method on `SessionType` and
      should be incorporated as a first-class method on each `TransportImpl`
      variant, paralleling the handling of `can_be_looked`, `can_be_written`,
      and `can_stream_output`.
- [ ] 7.2 No cross-proposal sequencing concern: `bundle_name` was removed by
      `retire-bundle-name-from-request-params` and `ui_session_id` is removed by
      task 1.1 above. `ChoicesPick` carries no reserved params after this proposal
      lands.
