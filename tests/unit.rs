#[path = "unit/acp/replay_coalescence.rs"]
mod acp_replay_coalescence;
#[path = "unit/acp/replay_tool_call_lifecycle.rs"]
mod acp_replay_tool_call_lifecycle;
#[path = "unit/acp_transport.rs"]
mod acp_transport;
#[path = "unit/adapter_binding.rs"]
mod adapter_binding;
#[path = "unit/adapter_choice.rs"]
mod adapter_choice;
#[path = "unit/adapter_clock.rs"]
mod adapter_clock;
#[path = "unit/adapter_frame.rs"]
mod adapter_frame;
#[path = "unit/adapter_handshake.rs"]
mod adapter_handshake;
#[path = "unit/adapter_lifecycle.rs"]
mod adapter_lifecycle;
#[path = "unit/adapter_look.rs"]
mod adapter_look;
#[path = "unit/adapter_persist.rs"]
mod adapter_persist;
#[path = "unit/adapter_script.rs"]
mod adapter_script;
#[path = "unit/adapter_submit.rs"]
mod adapter_submit;
#[path = "unit/association.rs"]
mod association;
#[path = "unit/config/mod.rs"]
mod config;
#[path = "unit/delivery_executor.rs"]
mod delivery_executor;
#[path = "unit/delivery_fence.rs"]
mod delivery_fence;
#[path = "unit/delivery_message.rs"]
mod delivery_message;
#[path = "unit/delivery_protocol.rs"]
mod delivery_protocol;
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
#[path = "unit/shutdown_clock.rs"]
mod shutdown_clock;
#[path = "support/mailbox.rs"]
mod stub_mailbox;
#[path = "unit/tmux_transport.rs"]
mod tmux_transport;
#[path = "unit/tui.rs"]
mod tui;
#[path = "unit/tui_binding_vocabulary.rs"]
mod tui_binding_vocabulary;
#[path = "unit/tui_bindings.rs"]
mod tui_bindings;
#[path = "unit/tui_dispatch.rs"]
mod tui_dispatch;
#[path = "unit/tui_effective_bindings.rs"]
mod tui_effective_bindings;
#[path = "unit/tui_help/mod.rs"]
mod tui_help;
#[path = "unit/tui_reachability.rs"]
mod tui_reachability;
#[path = "unit/tui_relay_error_mapping.rs"]
mod tui_relay_error_mapping;
#[path = "unit/tui_session.rs"]
mod tui_session;
#[path = "unit/tui_workbench/mod.rs"]
mod tui_workbench;
#[path = "unit/ui_transport.rs"]
mod ui_transport;
