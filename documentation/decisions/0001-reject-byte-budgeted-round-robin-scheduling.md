# 0001. Reject byte-budgeted round-robin scheduling

- Date: 2026-08-30
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: delivery-quiescence / Async Queue Lifecycle and Ordering

## Decision

Cross-target scheduling fairness is out of scope. Each target is served by its
own worker and the relay does not arbitrate between targets. No rotation,
credit, or per-visit budget is specified, and none may be introduced without
first naming the resource being allocated and the fairness guarantee being
offered.

## What we rejected, and why

An earlier revision specified byte-budgeted round-robin with a configured
quantum. It was withdrawn as circular:

- the quantum had to be at least the largest permitted handover byte
  component, while batch formation was already capped at that same component;
- so one quantum always afforded at least one full batch, and the credit could
  constrain only a *second* batch within a single visit;
- visits existed only because a rotation existed.

The budget was there to be fair within a rotation, and the rotation was there
to allocate the budget. Neither justified the other.

## Targets do contend — that is not an argument for restoring it

Real contention exists, and it is worth being precise that none of it is what
the rejected design measured:

- Tmux targets in one bundle share a single tmux server and socket.
- ACP bootstrap enters a shared blocking pool.
- A transport whose write seam blocks can occupy a delivery-runtime worker
  thread.

A global byte quantum represents none of these — not runtime occupancy, not
channel slots, not tmux-server capacity. A resource-grounded policy would be
denominated per shared resource and would state a throughput or fairness
objective. No such objective is required today.

The third item is a contract violation to be repaired at its source, not a
load to be scheduled around: the transport contract requires non-blocking
handover.

## Why this record exists

The rejected design is attractive on its face and the circularity is not
obvious until the two caps are compared. The specification that carried this
reasoning said so directly — the reason was recorded "because the mistake is
easy to repeat."
