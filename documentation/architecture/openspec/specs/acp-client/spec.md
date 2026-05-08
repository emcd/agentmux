# acp-client Specification

## Purpose
TBD - created by archiving change add-acp-client-tui. Update Purpose after archive.
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

The shared ACP client module SHALL expose a non-draining accessor that
returns a snapshot of the live replay buffer entries without consuming
or removing them. This accessor serves the relay look path, which
requires repeated reads of the same buffer state without disturbing
other consumers.

The existing draining accessor (which returns and removes entries)
SHALL be retained for non-look consumers such as the debug TUI binary
that intentionally drain entries on each iteration.

#### Scenario: Non-draining accessor returns current entries without consumption

- **WHEN** the relay look path calls the non-draining replay accessor
- **THEN** the call returns a snapshot of all currently-buffered replay entries in receive order
- **AND** the underlying buffer state is unchanged after the call

#### Scenario: Draining accessor remains available for debug TUI

- **WHEN** the debug TUI binary calls the draining replay accessor
- **THEN** the call returns and removes the buffered entries as it does today
- **AND** the non-draining accessor continues to function for relay look reads on other ACP clients

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

