# 0001. Reject byte-budgeted round-robin scheduling

- Date: 2026-08-30
- Status: accepted
- Supersedes: —
- Superseded by: —
- Specs: delivery-quiescence / Async Queue Lifecycle and Ordering

## Decision

The cross-target scheduler the proposal specified was withdrawn rather than
implemented, and no rotation, credit, or per-visit budget was introduced in its
place.

The standing rule this left behind — what a later proposal has to establish
before adding one — is normative, and lives in the `delivery-quiescence`
capability's `Async Queue Lifecycle and Ordering` requirement rather than in this
record.

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

An early defence of this withdrawal claimed that targets do not contend. That
claim was false, and correcting it did not change the conclusion.

The counterexamples raised at the time were the shared per-bundle tmux server
and socket, the shared blocking pool that ACP bootstrap enters, and a transport
whose write seam blocks occupying a delivery-runtime worker thread. The third
was treated not as load to be scheduled around but as a defect to repair at its
source, on the grounds that a blocking write seam contradicted the transport
contract as it then stood; it was repaired subsequently. The first two were
still live when this was recorded.

The conclusion survived on different ground. A global byte quantum measured none
of those things — not runtime occupancy, not channel slots, not tmux-server
capacity. The withdrawn proposal named no resource-grounded objective for the
quantum to serve, so there was no stated purpose the quantum was failing to
meet, and nothing to weigh the contention against. That was so whether or not
targets contend.

## Why this record exists

The rejected design is attractive on its face and the circularity is not
obvious until the two caps are compared. The specification that carried this
reasoning said so directly — the reason was recorded "because the mistake is
easy to repeat."
