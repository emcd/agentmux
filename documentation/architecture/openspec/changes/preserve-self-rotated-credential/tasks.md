## 1. Exclusion

- [ ] 1.1 In `handle_change_psk` (`src/relay/handlers/identity.rs`), skip the
  `revoke_streams_for_identity` sweep when the requester is rotating its own
  principal, leaving the trusted-host `identity.revoked` fan-out unconditional.
- [ ] 1.2 Record at the call site why skipping the sweep cannot spare a third
  party: the stream registry keys one entry per `principal_id`, so a
  self-rotation's only possible match is the requester's own connection. Name the
  registry keying, not just the conclusion, so a change that re-keys the registry
  has a reason to find this.
- [ ] 1.3 Update the `handle_change_psk` doc comment, which currently states
  without qualification that any connection authenticating with the old
  credential is force-disconnected.

## 2. Self-Rotation Test

- [ ] 2.1 Add a self-rotation test to
  `tests/unit/relay_stream/identity/psk_lifecycle.rs` that provisions the
  requester with a real PSK and authenticates its Hello with it. A socket-trust
  requester carries no `authenticated_identity` and cannot reach the defect, so a
  test written against the harness default would pass on unfixed code.
- [ ] 2.2 Assert the rotation response arrives on the requesting connection and
  carries a PSK distinct from the original.
- [ ] 2.3 Read the expected response directly rather than draining to EOF. A
  drain-to-EOF probe waits out the frame-wait budget once the connection
  correctly stays open, which costs seconds per run and asserts nothing.
- [ ] 2.4 Add a test that a self-rotation still fans `identity.revoked` out to
  in-scope trusted hosts, so the requester exclusion cannot silently widen from
  the teardown to the fan-out.

## 3. Third-Party Control

- [ ] 3.1 Confirm `change_psk_revokes_live_session_holding_old_credential` still
  covers third-party revocation under the exclusion, and extend it only if it
  does not distinguish the requester from the revoked session.
- [ ] 3.2 Verify the third-party test is a working control for the self-rotation
  assertion: with the sweep disabled outright, the self-rotation test passes and
  the third-party test fails.

## 4. Teeth-Checks

- [ ] 4.1 Revert only task 1.1 and confirm the self-rotation test fails on the
  missing rotation response, not on a timeout or a harness error.
- [ ] 4.2 Check each new assertion individually rather than trusting one failing
  run: a test stops at its first failed assertion and proves nothing about the
  ones after it.
- [ ] 4.3 Run the self-rotation test repeatedly to confirm the fix is
  deterministic rather than merely favoured, since the defect it covers was a
  race that the unfixed code won twice in fifty runs.

## 5. Suite

- [ ] 5.1 Run the full `relay_stream::identity` cluster.
- [ ] 5.2 Run the full unit and integration suites under `timeout`.
- [ ] 5.3 Run `cargo clippy` and `cargo fmt --check`.
