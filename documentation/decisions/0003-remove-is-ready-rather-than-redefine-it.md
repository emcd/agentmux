# 0003. Remove `is_ready` rather than redefine it

- Date: 2026-08-30
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: transport-abstraction / Transport Handover Capacity and Readiness

## Decision

When the transport contract needed a readiness predicate that meant "a handover
can be taken now", the existing `is_ready` was removed from the contract rather
than redefined to carry that meaning.

## What we rejected, and why

Keeping the name and changing what it meant.

`is_ready` answered a weaker question — whether a transport's machinery
existed. At the time, Tmux answered it unconditionally true, and ACP and Pty
counted a busy target as ready. That is why it could not serve handover
readiness.

Redefinition was rejected because **the two readiness questions were confusable
precisely under a name that does not say what the target is ready *for***.
Keeping the name would have left every existing call site reading plausibly and
behaving differently — a silent change with no compiler assistance. Removing it
forces each site to be revisited.

## What survives

The rejection was of a *contract* predicate with an underspecified name, not of
the underlying question. A transport may keep an equivalent lifecycle predicate
privately, and Pty did so at the time of this decision, so that inspection
still reached a target that was mid-turn.

## The general lesson

A predicate whose name omits what it is ready *for* will accumulate
incompatible readings. The same instinct is why target health is named and
specified separately from readiness: "not ready" and "not reachable" are
different facts, and one name covering both would be re-litigated indefinitely.
