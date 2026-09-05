# Capability Specifications

Each subdirectory here is one live capability holding a single `spec.md`. There
is deliberately no roster of them in this file: a list of capabilities is wrong
the moment one is added or retired, and it drifts silently because nothing reads
it. `ls` answers that question, and

```
grep -h '^### Requirement:' openspec/specs/*/spec.md | wc -l
```

answers how many requirements the corpus holds, without a number here going
stale between archives.

## Where a session-relay requirement lives

Nine of these capabilities were partitioned out of a former single-file
`session-relay` specification. What each covers is the part a reader cannot
infer from the directory name:

| Capability | Covers |
|------------|--------|
| `addressing-routing` | Canonical IDs, namespace semantics, target resolution, list payloads |
| `delivery-quiescence` | Send envelope, async queue lifecycle, terminal outcomes, ack semantics, and asynchronous terminal-outcome receipt |
| `transport-contracts` | Per-transport execution contracts (tmux, ACP, Pty): worker lifecycles, transport capability flags, copy-mode-transparent injection, and inter-transport error codes |
| `authorization-scope` | Policy presets, authorization vocabulary and evaluation, scope controls, uniform cross-bundle auth, UI sender validation, and per-operation authorization mappings |
| `look-and-stream-events` | Look operation (transport-agnostic and per-transport), persistent client streams, Hello registration, recipient routability, and stream event contracts |
| `choice-decisions` | Choice/decision envelope, queue lifecycle, operator classes |
| `bundle-lifecycle` | Reconciliation, bundle up/down, startup health, file watching |
| `environment-variables` | Coder/bundle/session environment variable precedence and container-injected overrides |
| `raww` | The relay-side semantic contract for the `raww` verb: operation, target resolution and bundle boundary, authorization mapping, transport behavior, response, input bounds |

A requirement in one of these domains is authored in the capability that matches
it. The remaining capabilities in this directory were never part of that
partition and stand on their own.

## Finding the code for a capability

There is no directory-per-capability in `src/`, and looking for one will mislead.
The source tree is decomposed by stage in the request path; these specifications
are decomposed by observable contract. Those are orthogonal axes, so a capability
defined as a stage lands in one module while a capability defined by its subject
is spread across every stage it touches.

`src/relay/README.md` explains this under "How this tree relates to the
specifications", including which shape a given capability has and why two
capabilities may legitimately reach the same file.

## Delta paths

A change's delta for a requirement is authored at
`<change>/specs/<capability>/spec.md`, naming the capability that currently
holds the requirement. `opsx-sync` resolves MODIFIED deltas and adds ADDED
requirements by requirement name, so the delta body is portable across paths and
only the containing directory changes -- which is what makes a stale path easy
to miss, since the text in it still reads correctly.

The full rule, including what goes wrong when a capability is partitioned or
renamed underneath an in-flight change, is `agentmux:procedures/general/7`.

## Reserved names

`runtime-api` is reserved for the embeddable runtime API capability owned by the
`embeddable-runtime-api` change; do not create it here before that change
archives. The change's own delta spec is the authority on what it will contain,
and a copy of that here would drift with every edit the change makes.
