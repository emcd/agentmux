---
name: "rebase-sweep"
description: "Idle-check all bundle sessions, then rebase all worktrees onto master. Run before restarting the relay so MCP servers rebuild from current code."
allowed-tools: "Bash, ToolSearch, mcp__agentmux__list, mcp__agentmux__look"
---
## Purpose

Prepare for a relay restart by verifying every agent session is idle and
rebasing every git worktree onto master. This ensures that when the relay
restarts and each session's MCP server rebuilds, it compiles against the
current codebase — not a stale pre-merge snapshot.

Run this skill whenever master has advanced significantly (landed lanes,
archived proposals) and a relay restart is imminent.

## Process

### 1. Load MCP tools

Use `ToolSearch` to load `mcp__agentmux__list` and `mcp__agentmux__look` if
they are not already available in the session.

### 2. Discover all bundles and sessions

Call `mcp__agentmux__list` with `command="principals"` once without a namespace
to get the default bundle, then again with each known aux namespace
(e.g. `agentmux-aux`) to discover hosted bundles. Collect every session whose
bundle `state` is `"up"`.

Do not hardcode the bundle list — iterate what the relay reports.

### 3. Look at every session in parallel

Call `mcp__agentmux__look` with `lines=5` for every non-self session across all
bundles simultaneously. Classify each result:

- **Idle (Tmux/Opencode)**: snapshot shows a Build/wait screen, or a bare shell
  prompt with 0 % quota. Safe to proceed.
- **Idle (Claude Code)**: snapshot shows `❯` prompt at 0 % quota. Safe.
- **ACP session**: snapshot shows ACP replay entries. Read the most recent entry.
  If the last entry is a completed tool call or an agent message with no pending
  work, treat as idle. If a tool call shows `status: "pending"` or the agent
  line describes active work, flag as active.
- **Empty snapshot**: treat as idle (session started but nothing rendered yet).

### 4. Gate on idle confirmation

If any session is classified as **active**, stop and report it to the operator
with the relevant snapshot lines. Do not proceed with rebasing until the operator
confirms it is safe. List all active sessions before pausing — do not report
one at a time.

If all sessions are idle, report the idle summary and continue.

### 5. Enumerate worktrees

Run `git worktree list` from the repository root. Collect every worktree path
whose branch is not `master` (i.e. skip the main checkout).

### 6. Rebase each worktree onto master

For each non-master worktree, run:

```
cd <worktree-path> && git rebase master
```

Run these in parallel. If any rebase fails (conflict or error), report it
immediately with the error output; do not abort the remaining rebases.

### 7. Report results

After all rebases complete, run `git worktree list` once more and display the
final state. Flag any worktrees whose commit hash differs from master HEAD —
these either have legitimate local branch commits (note them) or hit a rebase
failure (escalate them).

Also note any sessions that had unexpected activity during the look step, as
a situational awareness summary for the operator.

## Guardrails

- Never rebase the main checkout (`[master]` worktree). Only rebase branch
  worktrees.
- Do not skip the idle-check gate. The rebase is safe only when sessions are
  not mid-task.
- Do not force-push or reset any branch. `git rebase master` only — if a
  worktree has conflicts, report and leave it for the lane owner to resolve.
- If a session's look returns an error (relay unavailable, target not found),
  treat it as unknown — report it and ask the operator before proceeding.
- ACP sessions require human judgment for ambiguous snapshots. When in doubt,
  flag rather than classify as idle.
