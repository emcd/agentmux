# Prompt-Readiness Manual Smoke Test

This procedure validates that relay delivery:

- times out while a target session is quiescent but not prompt-ready, and
- delivers after prompt-readiness text appears.

It uses:

- one relay process,
- one MCP sender session (`sender`), and
- one human-observable target session (`human`).

## 1. Prepare isolated paths

Run from repository root:

```bash
SMOKE_ROOT=".auxiliary/temporary/smoke/prompt-readiness"
CONFIG_ROOT="${SMOKE_ROOT}/config"
STATE_ROOT="${SMOKE_ROOT}/state"
BUNDLE_NAME="smoke-pr"
TMUX_SOCKET="${STATE_ROOT}/bundles/${BUNDLE_NAME}/tmux.sock"

mkdir --parents "${CONFIG_ROOT}/bundles" "${STATE_ROOT}"
```

## 2. Write bundle configuration

```bash
cat > "${CONFIG_ROOT}/coders.toml" <<'TOML'
format-version = 1

[[coders]]
id = "sender-coder"
initial-command = "sh -lc 'exec sleep 3600'"
resume-command = "sh -lc 'exec sleep 3600'"

[[coders]]
id = "human-coder"
initial-command = "sh -lc 'exec sleep 3600'"
resume-command = "sh -lc 'exec sleep 3600'"
prompt-regex = "READY>"
prompt-inspect-lines = 8
TOML

cat > "${CONFIG_ROOT}/bundles/${BUNDLE_NAME}.toml" <<'TOML'
format-version = 1

[[sessions]]
id = "sender"
name = "sender"
display-name = "Sender"
directory = "/tmp"
coder = "sender-coder"

[[sessions]]
id = "human"
name = "human"
display-name = "Human"
directory = "/tmp"
coder = "human-coder"
TOML
```

## 3. Create tmux sessions on the bundle socket

```bash
tmux -S "${TMUX_SOCKET}" new-session -d -s sender "sh -lc 'exec sleep 3600'"
tmux -S "${TMUX_SOCKET}" new-session -d -s human "sh -lc 'printf \"BOOTING\\n\"; exec sleep 3600'"
```

Optional observer terminal:

```bash
tmux -S "${TMUX_SOCKET}" attach-session -t human
```

## 4. Start relay

Terminal A:

```bash
cargo run --bin tmuxmux-relay -- \
  --bundle "${BUNDLE_NAME}" \
  --config-directory "${CONFIG_ROOT}" \
  --state-directory "${STATE_ROOT}"
```

Expected startup line includes:

- `tmuxmux-relay listening`
- `bundle=smoke-pr`

## 5. Start MCP server bound to sender session

Terminal B:

```bash
cargo run --bin tmuxmux-mcp -- \
  --bundle-name "${BUNDLE_NAME}" \
  --session-name sender \
  --config-directory "${CONFIG_ROOT}" \
  --state-directory "${STATE_ROOT}"
```

## 6. Exercise `chat` from the MCP client

Use any MCP client connected to the process in Terminal B.

1. Send targeted chat while `human` pane only contains `BOOTING`.
   Request:

   ```json
   {
     "request_id": "smoke-timeout-1",
     "message": "first message",
     "targets": ["human"],
     "broadcast": false
   }
   ```

   Expected outcome:

   - response status is `failure`,
   - target outcome is `timeout`,
   - timeout reason mentions prompt readiness mismatch.

2. Mark target session as prompt-ready:

   ```bash
   tmux -S "${TMUX_SOCKET}" send-keys -t human -- "printf 'READY>\\n'" Enter
   ```

3. Send targeted chat again.
   Request:

   ```json
   {
     "request_id": "smoke-deliver-1",
     "message": "second message",
     "targets": ["human"],
     "broadcast": false
   }
   ```

   Expected outcome:

   - response status is `success`,
   - target outcome is `delivered`.

4. Verify injection reached `human` pane:

   ```bash
   tmux -S "${TMUX_SOCKET}" capture-pane -p -t human -S -80
   ```

   Expected pane output includes the RFC822/MIME envelope and `second message`.

## 7. Cleanup

```bash
tmux -S "${TMUX_SOCKET}" kill-server || true
rm --recursive --force "${SMOKE_ROOT}"
```
