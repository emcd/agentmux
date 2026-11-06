#[path = "integration/acp/mod.rs"]
mod acp;
#[path = "integration/adapter_conformance/mod.rs"]
mod adapter_conformance;
#[path = "integration/adapter_pipes.rs"]
mod adapter_pipes;
#[path = "integration/adapter_pipes_handshake.rs"]
mod adapter_pipes_handshake;
#[path = "integration/adapter_pipes_oversize.rs"]
mod adapter_pipes_oversize;
#[path = "integration/adapter_pipes_stall.rs"]
mod adapter_pipes_stall;
#[path = "integration/adapter_r2_admission.rs"]
mod adapter_r2_admission;
#[path = "integration/adapter_r2_admission_errors.rs"]
mod adapter_r2_admission_errors;
#[path = "integration/adapter_r2_admission_exemptions.rs"]
mod adapter_r2_admission_exemptions;
#[path = "integration/adapter_r2_framing.rs"]
mod adapter_r2_framing;
#[path = "integration/adapter_r2_look_liveness.rs"]
mod adapter_r2_look_liveness;
#[path = "integration/adapter_r2_payload.rs"]
mod adapter_r2_payload;
#[path = "integration/adapter_r2_replies.rs"]
mod adapter_r2_replies;
#[path = "integration/adapter_r2_saturation.rs"]
mod adapter_r2_saturation;
#[path = "integration/adapter_r2_scheduling.rs"]
mod adapter_r2_scheduling;
#[path = "integration/adapter_r2_submit_outbox.rs"]
mod adapter_r2_submit_outbox;
#[path = "integration/adapter_r2_wrong_method.rs"]
mod adapter_r2_wrong_method;
#[path = "integration/adapter_stdio.rs"]
mod adapter_stdio;
#[path = "integration/cli/mod.rs"]
mod cli;
#[path = "integration/mcp/mod.rs"]
mod mcp;
#[path = "integration/relay_delivery_dwell.rs"]
mod relay_delivery_dwell;
#[path = "integration/runtime_bootstrap/mod.rs"]
mod runtime_bootstrap;
#[path = "integration/session_relay_delivery.rs"]
mod session_relay_delivery;
#[path = "integration/session_relay_list.rs"]
mod session_relay_list;
#[path = "integration/session_relay_look.rs"]
mod session_relay_look;
#[path = "integration/session_relay_reconciliation.rs"]
mod session_relay_reconciliation;
#[path = "integration/session_relay_revocation_recovery.rs"]
mod session_relay_revocation_recovery;
#[path = "integration/session_relay_stream/mod.rs"]
mod session_relay_stream;
#[path = "integration/support/mod.rs"]
mod support;
#[path = "integration/transport_generation_fence.rs"]
mod transport_generation_fence;
