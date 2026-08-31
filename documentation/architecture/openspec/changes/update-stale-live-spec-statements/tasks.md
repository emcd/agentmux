## 1. Sync the requirement deltas

- [ ] 1.1 Sync `runtime-bootstrap` — `Relay Configuration File` gains the
      `unreachable-dwell-ms` row; confirm the existing "Every range above
      excludes zero" sentence and the "`unreachable-dwell-ms` is not an
      exception to this" paragraph now both hold against the table
- [ ] 1.2 Sync `addressing-routing` — `Session Type Taxonomy` reads five types
      with the `Pty` row and the new derivation scenario
- [ ] 1.3 Sync `look-and-stream-events` — `ACP Look Snapshot Contract` no longer
      describes `take_replay_entries` as available, and its eleven `\'`
      sequences are corrected
- [ ] 1.4 Sync `relay-routing-layer` — `Cross-Relay Target Ingress Filter` no
      longer calls `on_behalf_of` reserved or deferred, drops "this slice", and
      carries the new attribution-does-not-widen-ingress scenario
- [ ] 1.5 Sync `tui-surface` — `Initial TUI Workflow Coverage` cites
      `look-and-stream-events` rather than the archived change
- [ ] 1.6 Sync `cli-surface` — `CLI raww actor identity resolution` reads
      `users.toml`
- [ ] 1.7 Sync `authorization-scope` — `UI Request-Path Sender Validation` reads
      `users.toml`

## 2. Direct prose edits (no delta mechanism governs these)

- [ ] 2.1 `specs/cli-surface/spec.md` Purpose preamble: replace the `tui.toml`
      reference with `users.toml` for session selection and `ui.toml` for bundle
      selection, matching how the rest of the capability describes them
- [ ] 2.2 `specs/session-relay/spec.md` hub preamble: correct the
      "session type taxonomy (tmux/acp/ui/pubsub)" echo to include `pty`

## 3. Verify

- [ ] 3.1 `scripts/verify-openspec-deltas.py update-stale-live-spec-statements`
      reports zero errors and zero dropped scenarios, re-run immediately before
      sync in case a live spec moved underneath a delta
- [ ] 3.2 `openspec validate --all --strict` passes
- [ ] 3.3 Confirm no `tui.toml` reference remains anywhere under
      `openspec/specs/`
- [ ] 3.4 Confirm no live spec still cites an archived change as normative
      authority for these requirements
- [ ] 3.5 Confirm the four-session-type claim survives nowhere: sweep
      `openspec/specs/` for "four session types" and for
      taxonomy lists omitting `pty`
- [ ] 3.6 Spot-check that each synced requirement retains every scenario it had
      before, since `openspec validate` cannot see retention

## 4. Close out

- [ ] 4.1 Mark `todos/openspec/3` complete
- [ ] 4.2 Mark `todos/general/34` complete
- [ ] 4.3 File the residual finding: `todos/openspec/4` (allowed-scope cap) and
      `todos/openspec/7` (retire `session-relay`) remain open and are
      deliberately out of scope here
