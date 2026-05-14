# Manual ACP Permission-Flow Test Plan

Reference: task 5.8 of `add-acp-permission-request-ui-mediated`.

First validated: 2026-05-14 (e2e session with pty worktree).

## Prerequisites

- Live relay running with operator policy active for `user` TUI session.
- `pty` worktree open in ACP-hosted mode (opencode wrapping).
- TUI open and connected as `user` session.

## Steps

### 1. Trigger permission request

1. From `user` TUI session, send a prompt to `pty` via `agentmux send`.
2. Instruct pty agent to `touch` a new file under `.auxiliary/scribbles/`.
3. Relay routes `session/request_permission` from opencode to the relay.

### 2. Observe TUI display

4. Confirm permission choices appear in the TUI.
   - **Current behavior (known gap):** choices appear in the chat screen, not in
     the Look overlay. Bug tracked under tasks 4.4 and 5.7.
   - **Target behavior:** choices appear in Look overlay for the pty session,
     filtered to that session's pending requests only.

### 3. Approve via TUI

5. Select the approve/allow action from the displayed choices.
6. Confirm the relay receives `permission.resolve` with `outcome=selected` and
   explicit `option_id`.
7. Confirm opencode resumes and the file is created (tool call completes).

### 4. Cancel via TUI

8. Repeat step 1–3 with a new file.
9. Select the cancel/deny action.
10. Confirm opencode receives `cancelled` outcome and the tool call does not
    complete.
11. Confirm sender-visible terminal outcome is `failed` with
    `reason_code=runtime_permission_request_cancelled`.

### 5. Relay restart durability

12. Restart live relay while a permission request is pending.
13. Confirm pending request is restored from `permission_queue.json` on restart.
14. Confirm `permission.snapshot` is replayed to re-connecting TUI session.
15. Confirm approval still works after restart.

## Known gaps at time of first validation

- TUI shows choices in chat screen instead of Look overlay (tasks 4.4, 5.7).
- Full ACP `options[]` metadata passthrough not yet verified end-to-end (task 5.6).
- Escape-key handling in TUI accidentally cancels requests (UX issue, no tracking
  task yet).
