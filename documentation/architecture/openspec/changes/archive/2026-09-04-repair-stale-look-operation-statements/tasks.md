## 1. Confirm each replaced statement against the implementation

- [x] Bare look target is rejected with `validation_unqualified_target` and is
      never resolved against the requester's bound bundle
- [x] `RelayRequest::Look` carries no `bundle_name` field
- [x] A relay-wide look target takes the relay-wide branch and yields
      `validation_unknown_target` or `validation_unsupported_operation`
- [x] A successful peer-bundle look response carries a qualified
      `target_session` and no `bundle_name`

## 2. Check the surfaces still agree after the repair

- [x] `cli-surface` and `mcp-tool-surface` look requirements do not restate the
      bare-target allowance being withdrawn here
- [x] No other live requirement names a `bundle_name` look field in either
      direction

`mcp-tool-surface` says explicitly that the MCP server qualifies a bare target
before the relay call, which is consistent with the relay rejecting one.
`cli-surface` says a bare target "MAY be a bare session id (inspected within the
requester's dispatch bundle)" without saying the CLI qualifies it first. That is
accurate about the CLI's accepted input and does not contradict the repaired
relay requirement, but it would read better in the `mcp-tool-surface` form.
Noted rather than changed: it is not a defect, and widening this change to
polish it is the drift this repair was meant to reverse.

## 3. Cover the rejections that had no test

Not done, and deliberately not blocking the archive. Both scenarios assert
behavior the handlers already have; the repair corrects statements that were
wrong about existing code rather than specifying anything new. Carried forward
as `agentmux:todos/openspec/18`.

- [ ] Assert a bare look target is rejected at the relay rather than resolved
- [ ] Assert a relay-wide look target is rejected by the capability check and
      not as an unknown bundle
