use std::env;

fn main() {
    if let Err(err) = agentmux::commands::run_agentmux_relay_legacy(env::args().skip(1).collect()) {
        agentmux::runtime::inscriptions::emit_inscription(
            "relay.startup_failed",
            &serde_json::json!({ "error": err.to_string() }),
        );
        eprintln!("agentmux-relay: {err}");
        std::process::exit(1);
    }
}
