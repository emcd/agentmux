# Design: Remove the operator-interaction delivery gate

## Context

The operator-interaction gate was introduced when the Tmux transport injected
messages with `tmux send-keys`. In copy-mode, `send-keys` input is routed
through the copy-mode key table and interpreted as copy-mode *commands* rather
than delivered to the child process — so injecting into a pane the operator had
scrolled would both fail to deliver and corrupt the operator's copy-mode
session. Suppressing delivery until the interaction cleared was the right call
for that mechanism.

The body injection has since moved to `paste-buffer`. The gate was never
revisited. This document records the measurements that justify deleting it, so
a future reader does not have to re-derive them — and so nobody reintroduces
`send-keys` on the injection path without noticing what it costs.

## Decision 1: Delete the gate rather than bound it

**Rejected: bound the suppression with a timeout.** A bounded gate still drops
or defers a message that could have been delivered successfully, and it requires
inventing a new terminal reason code for "the operator was scrolling." It
preserves a restriction that no longer has a mechanism behind it, and it leaves
the door open to the same class of silent stall with a longer fuse.

**Chosen: delete the gate.** The measurements below show delivery into a
copy-mode pane is both *possible* and *non-disruptive*, and that classification
is unaffected by copy-mode. There is nothing left for the gate to protect.

### Measurement A — what survives copy-mode

Method: a tmux 3.4 server on a private socket; pane running `cat > out.txt`, so
any byte the *child* receives is observable in `out.txt`. Pane put into
copy-mode with `copy-mode -u` (what a mouse-wheel scroll does).
`#{pane_in_mode}` confirmed `1` before each attempt.

| Mechanism | Child receives it? | Pane left in copy-mode? |
| --- | --- | --- |
| `paste-buffer -d -p -b <buf> -t <pane>` (body) | **Yes** | Yes — undisturbed |
| `send-keys -t <pane> Enter` (submit) | **No** | Yes |
| `send-keys -H -t <pane> 0d` (raw CR byte) | **No** | Yes |
| `paste-buffer` carrying a bare `\r` (unbracketed) | **Yes** | Yes — undisturbed |

`paste-buffer` writes directly to the pane's pty and never consults the
copy-mode key table. `send-keys` synthesizes a *key*, which the copy-mode key
table intercepts — and `-H` does not escape that routing, only the byte
encoding. The distinction is routing, not payload: the same `0x0d` byte
arrives when pasted and is swallowed when sent as a key.

> **Trap for the unwary.** The first run of this experiment showed `paste-buffer`
> apparently delivering *nothing*. That was the pty line discipline, not the
> paste: `cat` is in canonical mode and returns nothing until a newline arrives.
> Any repeat of this measurement must put the newline **inside** the buffer (or
> use a raw-mode child), or it will draw exactly the wrong conclusion.

### Measurement B — what the classifier sees while scrolled back

Method: pane with 60 lines of scrollback and a distinctive live prompt; scrolled
up 20 lines inside copy-mode.

| Probe | Not in copy-mode | Scrolled up 20 lines in copy-mode |
| --- | --- | --- |
| `capture-pane -p` (tail) | `LIVE_PROMPT>` | `LIVE_PROMPT>` |
| `#{cursor_x}` | `13` | `13` |

`capture-pane` reads the pane's underlying grid, not the copy-mode overlay. So
prompt-readiness — regex match on the captured tail plus `cursor_x` against
`prompt-idle-column` — is **identical** whether or not the operator is scrolled
back. Wedge and unresponsive classification therefore need no protection from
copy-mode: they were never seeing it.

This is also why the original incident was misdiagnosed as a cross-namespace
routing bug. `agentmux look` reported a perfectly healthy idle pane at its
prompt, because that is genuinely what the pane contained. The gate was the only
thing that knew otherwise, and after its first tick it said nothing.

## Decision 2: Move the submit to an unbracketed CR paste-buffer

The body is pasted with `-p` (bracketed paste) deliberately: bracketed paste is
what stops a multi-line message body from submitting at its first embedded
newline. That is why the submit was a separate `send-keys Enter` rather than a
trailing `\n` folded into the body — and it is why folding the newline back into
the body buffer is **not** an option. Inside a bracketed paste, a TUI that has
requested bracketed-paste mode treats the newline as literal text, so the
message would be typed but never sent.

The submit therefore needs to be a CR that arrives *outside* the paste brackets
but still bypasses the key table. A second, **unbracketed** `paste-buffer`
carrying a bare `\r` is exactly that (Measurement A, row 4): it reaches the pty
as a carriage return, the TUI treats it as Enter, and copy-mode never sees it.

Sequence, replacing the current body-paste-then-`send-keys`:

1. `load-buffer` + `paste-buffer -d -p` — message body, bracketed.
2. `load-buffer` + `paste-buffer -d` — a single `\r`, unbracketed.

Both steps are pty writes. The full injection path is then
copy-mode-transparent, which is the property the new
`Copy-Mode-Transparent Injection` requirement pins so a future change cannot
quietly reintroduce `send-keys` and resurrect this bug.

## Decision 3: Retire `PendingChoiceProbe`

The five canonical probe sequences include `PendingChoiceProbe`, whose asserted
behavior is "neither timeout nor wedge; the transport continues to wait
indefinitely while operator interaction is active and the prime timer does NOT
fire." That is a test for the bug. It goes with the gate.

There is no coverage gap. "Pending choice" as a *pane state* — an agent stopped
at a tool-approval dialog — is wedge-class content and is already asserted by
`AlwaysWedgeProbe`. What made `PendingChoiceProbe` distinct was solely the
operator-interaction flag, and pane classification never depended on the
operator: a pane stuck on a dialog is stuck whether or not someone is scrolling
it. Four canonical sequences remain: unresponsive, wedged, slow-prompt,
normal-flow.

## Cross-lane note (Pty)

`WedgeObservation` is shared across transports, so removing
`operator_interaction_active` reaches into the Pty lane. Pty confirmed it has no
semantic dependency: the field is hardcoded `false` at `src/pty/state.rs:304`
and `:460` and never set true, and the archived `add-pty-transport` design
already recorded the decision to keep it `false`-only ("For Pty, there is no
operator-attached TUI"). Pty has no copy-mode, no key-table, and routes every
write through the pty master — the mechanism the gate defended against does not
exist on that lane.

Per lane ownership, the two Pty struct-literal deletions are landed by the Pty
Specialist as a separate commit on this branch, so the shared-field removal and
its Pty cleanup merge atomically and no intermediate commit breaks the
`--features pty` build.
