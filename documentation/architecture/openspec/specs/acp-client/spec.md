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

