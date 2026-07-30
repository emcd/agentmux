#[path = "unit/acp/replay_coalescence.rs"]
mod acp_replay_coalescence;
#[path = "unit/acp/replay_tool_call_lifecycle.rs"]
mod acp_replay_tool_call_lifecycle;
#[path = "unit/acp_transport.rs"]
mod acp_transport;
#[path = "unit/association.rs"]
mod association;
#[path = "unit/config/mod.rs"]
mod config;
#[path = "unit/delivery_message.rs"]
mod delivery_message;
#[path = "unit/envelope.rs"]
mod envelope;
#[path = "unit/opencode_prompt_regex.rs"]
mod opencode_prompt_regex;
#[cfg(feature = "pty")]
#[path = "unit/pty_transport.rs"]
mod pty_transport;
#[path = "unit/relay/mod.rs"]
mod relay;
#[path = "unit/relay_stream.rs"]
mod relay_stream;
#[path = "unit/relay_stream_client.rs"]
mod relay_stream_client;
#[path = "unit/runtime_inscriptions.rs"]
mod runtime_inscriptions;
#[path = "unit/runtime_owned_relay.rs"]
mod runtime_owned_relay;
#[path = "unit/runtime_paths.rs"]
mod runtime_paths;
#[path = "unit/runtime_sockets.rs"]
mod runtime_sockets;
#[path = "unit/runtime_starter.rs"]
mod runtime_starter;
#[path = "unit/tmux_transport.rs"]
mod tmux_transport;
#[path = "unit/transports_quiescence.rs"]
mod transports_quiescence;
#[path = "unit/tui.rs"]
mod tui;
#[path = "unit/tui_relay_error_mapping.rs"]
mod tui_relay_error_mapping;
#[path = "unit/tui_session.rs"]
mod tui_session;
#[path = "unit/tui_workbench.rs"]
mod tui_workbench;
#[path = "unit/ui_transport.rs"]
mod ui_transport;
