# Source Notes

## Starter Template Embedding

Starter configuration templates are version-controlled and embedded into the
binary with `include_str!`:

- coders template: `data/configuration/coders.toml`
- bundle template: `data/configuration/bundle.toml`

Runtime startup copies these templates into configuration roots only when the
target files are missing.

## Relay Delivery Internals (MVP)

`chat` supports `delivery_mode=async` and `delivery_mode=sync`.

Async mode queue semantics:

- in-memory only (non-durable),
- FIFO ordering per target session,
- no dedupe/coalescing,
- no hard queue cap in MVP.

If relay exits or restarts, pending async queue entries are currently lost.
