## Context

An earlier pass proposed deleting `Authorization Hooks for Do and Find` on the
grounds that it reserved hooks for verbs that do not exist and therefore bound
nothing. That justification was wrong and should not be reused: the requirement
says "SHALL **reserve**", the controls are parsed, and the discards are
deliberate — so the SHALL is satisfied exactly as written. What is unsatisfiable
is the pair of scenarios describing denial by a map that is thrown away.

The requirement goes anyway, on a different and stronger ground supplied by the
operator: a configuration key must not be required unless something implements
it.

The two halves are not symmetric, which is why the earlier pass stalled. `do`
is nearly free to remove. `find` is a required key under
`deny_unknown_fields`, so removing it is a hard startup refusal for any
deployment that still carries it.

## Goals / Non-Goals

**Goals:**

- Remove both reservations from the corpus, and record why the requirement
  cannot come back piecemeal.
- Leave behind a rule that prevents the same shape recurring.

**Non-Goals:**

- Implementing `find` or `do`. Neither verb is proposed here; when either
  arrives it brings its control back with the check that consumes it.
- Completing the control vocabulary. It lists five controls where the loader
  parses eleven; that is a real defect and a separate change.
- Choosing the release this rides. Sequencing is the operator's call.

## Decisions

**Remove the requirement rather than keep the SHALL and drop its two
unsatisfiable scenarios.** Keeping it would leave code and spec in agreement and
the discards justified, which is why it was a live alternative. It loses to the
config-key principle: the requirement is what obliges `policies.toml` to carry
`find`, so keeping it keeps the cost the ruling identifies as the defect.

**Remove `find` and `do` in one change despite very different costs.** They
could be split, and the cheap half could land immediately. But they are one
requirement and one principle, and splitting would leave a requirement named
"for Do and Find" describing only one of them for however long the breaking half
waits. The sequencing constraint attaches to the release, not to the proposal.

**State the recurrence rule in the vocabulary requirement rather than as a new
requirement of its own.** The vocabulary is the list that obliges the config
file to carry a key, so the constraint on what may enter that list belongs to
it. A standalone requirement would bind the same thing at one remove and invite
a reader to satisfy the list without consulting it.

**Correct `look = self` here rather than leave it or split it out.** A MODIFIED
delta replaces the whole requirement, so re-stating the built-in default list
means re-asserting every line in it. Carrying forward a line already known to be
false is worse than the scope cost of fixing it. The same statement appears in
`look-and-stream-events` Relay Look Operation and is corrected there by
`repair-stale-look-operation-statements`, so that each requirement is edited by
exactly one in-flight change.

## Risks / Trade-offs

**A deployment upgrades the binary without updating `policies.toml`** → Startup
refuses with a parse error naming the unknown `find` key. This is the loud
failure direction, and the removal must therefore land with the template change
in the same release. There is no partial-credit path: `deny_unknown_fields`
means the key and the field are one atomic pair.

**The removed requirement is the only record that these hooks were once
reserved** → The archived change retains it, and the Reason and Migration
notes state why it went and what returning would require, so a future proposal
to implement `find` starts from the reasoning rather than rediscovering it.

**Two `let _ =` discards outlive the fields if the code half lags** → They carry
no comment explaining their purpose today, so a later reader would reasonably
delete them as dead code and silently retire a specified reservation. The code
task names them explicitly for that reason.
