# acp-client Specification

## Purpose
The ACP integration surface — covers both the standalone `agentmux-acp` TUI binary and the shared `src/acp/` module used by the relay delivery subsystem for ACP-backed target sessions. The spec governs ACP protocol initialization, session lifecycle (session/new + session/load), the background reader thread that ingests `session/update` notifications into the shared replay buffer, stdin writer serialization, and ACP permission handling. The relay-side and binary-side contracts share one canonical vocabulary so both surfaces stay in lockstep.
## Requirements
### Requirement: ACP Client Binary
The system SHALL provide a standalone `agentmux-acp` binary that connects directly to an ACP-compatible agent via stdio, initializes the ACP protocol, and provides an interactive text interface for sending prompts and viewing responses.

#### Scenario: Successful connection
- **WHEN** `agentmux-acp --command "opencode acp"` is invoked
- **THEN** the binary spawns the ACP agent, sends `initialize`, creates or loads a session, and presents an interactive TUI

#### Scenario: Session resumption
- **WHEN** `agentmux-acp --command "opencode acp" --session-id <id>` is invoked
- **THEN** the binary loads the existing session by ID rather than creating a new one

### Requirement: Interactive Prompt Interface
The agentmux-acp TUI SHALL accept text input and send it as a raw prompt to the ACP server, displaying the streamed response in the output area.

#### Scenario: Send prompt
- **WHEN** the user types text and presses Enter in the TUI input area
- **THEN** the text is sent as a `session/prompt` request to the ACP server
- **AND** the response is streamed into the conversation history area

#### Scenario: Input blocked during prompt
- **WHEN** a prompt is being processed by the ACP server
- **THEN** the TUI remains responsive (renders streaming output) but input is queued or blocked until the prompt completes

### Requirement: Conversation History Display
The agentmux-acp TUI SHALL display the full conversation history in a scrollable area with visual distinction between user messages and assistant responses.

#### Scenario: User message styling
- **WHEN** a user prompt is sent
- **THEN** the prompt text appears in the conversation history with a distinct background color for user messages

#### Scenario: Assistant message styling
- **WHEN** an assistant response is received
- **THEN** the response text appears in the conversation history with a distinct background color for assistant messages

### Requirement: Shared ACP Protocol Module
The ACP stdio client implementation SHALL be extracted into a shared `src/acp/` module accessible by both the relay delivery subsystem and the agentmux-acp binary.

#### Scenario: Relay uses shared module
- **WHEN** the relay delivers messages to an ACP target
- **THEN** it uses `AcpStdioClient` from the shared `src/acp/` module

#### Scenario: Client uses shared module
- **WHEN** the agentmux-acp binary connects to an ACP server
- **THEN** it uses `AcpStdioClient` from the shared `src/acp/` module

### Requirement: Clean Shutdown
The agentmux-acp binary SHALL cleanly terminate the ACP child process and restore the terminal on exit.

#### Scenario: Ctrl+C exit
- **WHEN** the user presses Ctrl+C in the TUI
- **THEN** the ACP child process is terminated, the terminal is restored to its original state, and the binary exits

#### Scenario: ACP process exits unexpectedly
- **WHEN** the ACP child process terminates before the user exits
- **THEN** the TUI displays an error message and exits cleanly

### Requirement: Non-Draining Replay Buffer Accessor

The shared ACP client module SHALL expose two read accessors over the in-memory
replay buffer. The underlying buffer is a continuous append log populated by the
background reader thread from `session/update` notifications and by the delivery
thread on outgoing user prompts.

**Snapshot accessor (non-consuming):** returns all current entries in receive
order without modifying the buffer. Used by the relay look path, which requires
repeated reads of the same state without disturbing other consumers.

**Cursor accessor (non-consuming):** takes a cursor position (`usize` offset
into the buffer) and returns all entries from that offset onward, along with the
new cursor value. Used by the `agentmux-acp` debug TUI binary to read only
entries that arrived since the last render iteration. The buffer is not mutated;
the caller advances its own cursor.

The draining accessor (`take_replay_entries`, which returned and removed
entries) is removed. All consumers SHALL use one of the two non-consuming
accessors above.

#### Scenario: Snapshot accessor returns current entries without consumption

- **WHEN** the relay look path calls the snapshot accessor
- **THEN** the call returns all currently-buffered replay entries in receive order
- **AND** the underlying buffer state is unchanged after the call

#### Scenario: Cursor accessor returns only entries since last read position

- **WHEN** the debug TUI binary calls the cursor accessor with offset N
- **THEN** the call returns entries from index N onward and a new cursor M >= N
- **AND** the underlying buffer is unchanged
- **AND** a subsequent call with cursor M returns only entries received after M

#### Scenario: Replay buffer updated immediately on outgoing user prompt

- **WHEN** the relay writes a user prompt to ACP stdin
- **THEN** a `ReplayEntry::User` is appended to the shared buffer immediately
- **AND** `look` reflects the submitted message before any `session/update`
  response arrives

### Requirement: Replay Buffer Cap and Eviction

The shared ACP client module SHALL enforce a maximum entry count on the
live replay buffer with oldest-evict-first semantics. The cap SHALL be
1000 entries, mirroring the prior persisted-path retention bound.

When ingesting a new replay entry would exceed the cap, the oldest
entry SHALL be evicted to maintain the bound.

#### Scenario: Evict oldest entry when buffer reaches cap

- **WHEN** the live replay buffer holds 1000 entries
- **AND** a new replay entry is ingested
- **THEN** the oldest entry is evicted
- **AND** the buffer continues to hold exactly 1000 entries
- **AND** the most recent entry is the newly ingested one

### Requirement: ACP Background Reader Thread

The shared ACP client module SHALL spawn a dedicated background reader thread
for each ACP child process immediately after the child is started. The reader
thread SHALL own the child's stdout fd for the lifetime of the session.

The reader thread SHALL loop using blocking `read_line` on `BufReader<ChildStdout>`
and dispatch each received JSON-RPC message by type:

- `session/update` notification: parse `ReplayEntry` items, append to the
  shared in-memory replay buffer in receive order, and record
  `last_acp_frame_observed_at`.
- `session/request_permission` request: enqueue the permission request and
  write the JSON-RPC response to ACP stdin via the shared stdin writer.
- Response matching the active turn-complete channel: signal turn completion
  and transition worker state to `Available`.
- Response matching a pending-request registry entry: send the response value
  on the registered oneshot channel.
- Any other notification: log via inscription and discard.

On reader thread exit (EOF, I/O error, or panic), the implementation SHALL:
1. Transition worker state to `Unavailable`.
2. Drain the pending-request registry and close all pending oneshot channels
   with an error.
3. Close the turn-complete channel if one is set.

#### Scenario: Reader appends session/update entries to replay buffer

- **WHEN** the ACP server sends a `session/update` notification at any time
- **THEN** the background reader appends the parsed replay entries to the
  shared in-memory buffer in receive order
- **AND** `look` on the session reflects the new entries without any
  additional request being issued

#### Scenario: Reader dispatches session/request_permission

- **WHEN** the ACP server sends a `session/request_permission` request while
  the worker is idle (no active prompt)
- **THEN** the background reader hands the request to the permission
  resolution path so a decision can be returned via the shared stdin writer
- **AND** does not drop the request due to absence of an active `request()` call

The public stream-event payload shape used by MCP consumer surfaces is
intentionally out of scope for this change and is tracked separately under
`agentmux:todos/acp/15`.

#### Scenario: Reader transitions worker to Unavailable on EOF

- **WHEN** the ACP child process exits and stdout is closed
- **THEN** the reader thread exits
- **AND** worker state transitions to `Unavailable`
- **AND** any pending oneshot receivers receive an error

### Requirement: ACP Stdin Writer Serialization

The shared ACP client module SHALL serialize all writes to ACP stdin through a
single shared `Arc<Mutex<ChildStdin>>`. All write contexts — delivery thread
(prompt, initialize, session/new, session/load), permission resolution (JSON-RPC
response to `session/request_permission`), and shutdown — SHALL acquire this
mutex before writing.

No caller SHALL write to ACP stdin without holding the mutex.

#### Scenario: Concurrent write contexts do not race

- **WHEN** the delivery thread is writing a prompt AND the permission
  resolution path is writing a `session/request_permission` response
  concurrently
- **THEN** one write completes before the other begins
- **AND** the ACP child process receives both writes without interleaving

#### Scenario: Write failure yields transport_unavailable

- **WHEN** a write to ACP stdin fails with an I/O error
- **THEN** the operation returns error code `transport_unavailable`
- **AND** worker state transitions to `Unavailable`

### Requirement: ACP Pending-Request Registry

The ACP background reader SHALL maintain a pending-request registry
(`HashMap<request_id, oneshot::Sender<Value>>`) for non-prompt requests that
require synchronous acknowledgment (`initialize`, `session/new`, `session/load`).

Before issuing a non-prompt request, the caller SHALL register its request-id
in the registry and wait on a oneshot channel. The reader SHALL look up the id
on receiving any JSON-RPC response and send the value to the waiting caller.

Fire-and-forget applies only to `session/prompt`; the turn-complete signal for
prompt is routed through a separate channel, not the registry.

#### Scenario: session/load response routed to waiting caller

- **WHEN** the delivery thread sends `session/load` with request-id 42
  and registers id 42 in the pending-request registry
- **THEN** the background reader receives the `session/load` response
- **AND** sends the response value to the caller's oneshot receiver
- **AND** removes id 42 from the registry

#### Scenario: Registry drained on reader thread exit

- **WHEN** the background reader thread exits (any cause)
- **THEN** all pending oneshot channels in the registry are closed with an error
- **AND** waiting callers receive an error indicating the transport is gone

