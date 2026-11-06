# Adapter conformance transcripts

Contract-derived supervisor-side frame scripts, replayed through the
harness double (`super::replay`) against the reference adapter binary.
Each file names its requirement plus capability file; `limit` carries
the Tier-2 payload-richness wording where it applies, else null.

## Step language

- `{"send": {...}}` — one JSON-RPC frame to the adapter. `$name`
  strings substitute wire ids captured by earlier expectations. With
  `expand_b64`, the pointed-to string field is filled at run time with
  deterministic base64 filler (byte count rounded down to a multiple of
  four), keeping large payloads out of the fixture files.
- `{"send_raw": "..."}` — one raw line, for malformed envelopes.
- `{"expect": {...}}` — the adapter's next frame must subset-match.
  Objects match by subset, arrays element-wise at equal length, scalars
  by equality. `capture_id_as` / `capture_correlation_as` record the
  frame's wire id / params correlation for `$name` substitution.
- `{"expect_quiet_ms": N}` — no frame within N ms (a closed pipe fails).
- `{"expect_eof": true}` — the pipe must close (binding termination).
- `{"close_input": true}` — deliver stdin EOF, keep reading.

## Binding

`bind.expect_hello_subset` is matched against the adapter's hello
params; `bind.hello_result` answers it, or `bind.hello_result_raw`
sends a verbatim line with `$hello_id` substituted for the observed
hello wire id (adversarial results only; the fixture keeps the
surrounding quotes, so `"id": "$hello_id"` replays as `"id": "1"`).

## Assumptions (confirmed or pending with Agents)

- Unscripted correlations answer Submitted; unscripted targets answer
  synthetic stale empty looks. Script-defined values beyond the
  normative fields are the script's business, never asserted here.
- Choice-raising turns complete with their scripted ack after the turn
  observes local Cancelled (needed by the error-admission transcripts).
- `adapter_script` names the required Agents-owned TOML script; the
  spawn argv/env pairing that selects it is confirmed separately, and
  the runner currently launches exactly `spawn.args` + `spawn.env`.
- Over-bound frames replay in the gate like every other transcript; the
  discard-vs-terminate dispute that once parked one in `pending/` was
  adjudicated defect and repaired on the implementation side.
- Slow-consumer backpressure beyond final-flush integrity needs a
  flood script and is recorded as future, not asserted here.
- Every `relay.look` carries numeric `prime_timeout_ms` (0 answers
  immediately with no hold); omitting it is a structural refusal per
  the wire schema, never an answered poll. The citation lint enforces
  this.
