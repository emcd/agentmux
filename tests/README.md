# Test Notes

Manual prompt-readiness smoke procedure:

- `tests/smoke/prompt_readiness_manual.md`

## Pipe-family platform coverage

The live-executable pipe tests (`adapter_pipes*.rs`,
`adapter_pipes_oversize.rs`, `adapter_pipes_stall.rs`,
`adapter_pipes_handshake.rs`) drive the built adapter binary over
real OS pipes. The configured CI test legs are ubuntu-latest
(x86_64), ubuntu-24.04-arm (aarch64), and macos-latest
(aarch64-apple-darwin), plus the ubuntu-latest pty-feature row.
Local or ad-hoc runs on other machines are independent
executions, not configured coverage.

They are Unix-only by construction, and Windows is explicitly out of
scope: the harnesses use Unix file descriptors and `libc::pipe`
directly (`std::os::unix`), and the binary under test depends on
Unix signal semantics. In particular, `agentmux-adapter` ignores
`SIGPIPE` at startup so a write to a closed stdout surfaces as
`EPIPE` and runs the dead-channel local terminalization (exit 2
with a `transport_dead` writer-event record) instead of signal
death. The ignore is defensive and idempotent, not evidence of
differing OS defaults. The closed-stdout test pins exactly this
behavior with a pre-closed read end and a held-silent stdin (no
EOF), so no platform in the matrix can pass it by accident of
scheduling.

Pipe tests assert termination and exit codes with generous
wall-clock margins; no test asserts on timing precision.
