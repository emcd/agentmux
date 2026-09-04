## 1. Confirm each replaced statement against the implementation

- [ ] Bare look target is rejected with `validation_unqualified_target` and is
      never resolved against the requester's bound bundle
- [ ] `RelayRequest::Look` carries no `bundle_name` field
- [ ] A relay-wide look target takes the relay-wide branch and yields
      `validation_unknown_target` or `validation_unsupported_operation`
- [ ] A successful peer-bundle look response carries a qualified
      `target_session` and no `bundle_name`

## 2. Check the surfaces still agree after the repair

- [ ] `cli-surface` and `mcp-tool-surface` look requirements do not restate the
      bare-target allowance being withdrawn here
- [ ] No other live requirement names a `bundle_name` look field in either
      direction

## 3. Cover the rejections that had no test

- [ ] Assert a bare look target is rejected at the relay rather than resolved
- [ ] Assert a relay-wide look target is rejected by the capability check and
      not as an unknown bundle
