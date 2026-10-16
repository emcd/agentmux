---
name: "sync-lanes"
description: "Idle-check all bundle sessions, then ask each lane to pull latest master and repopulate agentsmgr content. Run before restarting the relay so MCP servers rebuild from current code."
allowed-tools: "Bash, ToolSearch, mcp__agentmux__list, mcp__agentmux__look, mcp__agentmux__send"
---
## Purpose

Prepare for a relay restart by verifying every agent session is idle and
asking each lane to synchronize its clone onto current master. This ensures
that when the relay restarts and each session's MCP server rebuilds, it
compiles against the current codebase — not a stale pre-merge snapshot.

Sessions live in independent clones under `~/src/CLONES/agentmux/<name>`,
not git worktrees. The coordinator cannot write to these directories directly
(RO mount), so each lane pulls its own clone.

Run this skill whenever master has advanced significantly (landed lanes,
archived proposals) and a relay restart is imminent.

## Process

### 1. Load MCP tools

Use `ToolSearch` to load `mcp__agentmux__list`, `mcp__agentmux__look`, and
`mcp__agentmux__send` if they are not already available in the session.

### 2. Discover all bundles and sessions

Call `mcp__agentmux__list` with `command="principals"` to enumerate all hosted
bundles and their sessions. Collect every session whose bundle `state` is
`"up"` and whose `transport` is `"tmux"` or `"pty"` (skip `ui`/`pubsub`).

Do not hardcode the bundle list — iterate what the relay reports. Also skip
yourself (the coordinator) — you manage your own checkout.

### 3. Look at every session in parallel

Call `mcp__agentmux__look` with `lines=5` for every non-self session across all
bundles simultaneously. Classify each result:

- **Idle (Tmux/Opencode)**: snapshot shows a Build/wait screen, or a bare shell
  prompt with 0 % quota. Safe to proceed.
- **Idle (Claude Code)**: snapshot shows a `❯` prompt at 0 % quota. Safe.
- **ACP session**: snapshot shows ACP replay entries. Read the most recent entry.
  If the last entry is a completed tool call or an agent message with no pending
  work, treat as idle. If a tool call shows `status: "pending"` or the agent
  line describes active work, flag as active.
- **Empty snapshot**: treat as idle (session started but nothing rendered yet).

### 4. Gate on idle confirmation

If any session is classified as **active**, stop and report it to the operator
with the relevant snapshot lines. Do not proceed until the operator confirms
it is safe. List all active sessions before pausing — do not report one at a
time.

If all sessions are idle, report the idle summary and continue.

### 5. Send sync requests to each lane

For each non-self session, send a message via `mcp__agentmux__send`:

```
Please synchronize your clone: run `git pull --rebase origin master` in your
session directory, then `agentsmgr populate project
'github:emcd/agents-common@master#distribution' <your-clone-path> --no-simulate`.
Report "synced" when done.
```

Send these in parallel (single `send` call with multiple targets). The
message should be terse — lanes know their own clone paths.

### 6. Wait for confirmations

Wait for each lane to respond with "synced" (or equivalent). If a lane reports
a rebase conflict, report it to the operator immediately — do not attempt to
resolve it from the coordinator. If a lane does not respond within a
reasonable window, flag it as unresponsive.

### 7. Verify

After all lanes confirm, optionally spot-check by looking at one or two
sessions to confirm they restarted their MCP servers cleanly. Report the
final state to the operator.

## Guardrails

- Do not skip the idle-check gate. Syncing is safe only when sessions are
  not mid-task.
- Do not attempt to `git pull` in lane directories directly — they are RO
  mounts from the coordinator's container. Each lane pulls its own clone.
- If a session's look returns an error (relay unavailable, target not found),
  treat it as unknown — report it and ask the operator before proceeding.
- ACP sessions require human judgment for ambiguous snapshots. When in doubt,
  flag rather than classify as idle.
- If a lane reports a rebase conflict, leave it for the lane owner to resolve.
  Do not force-push or reset any branch.
