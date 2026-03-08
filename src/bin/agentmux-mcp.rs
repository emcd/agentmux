use std::env;

#[tokio::main]
async fn main() {
    if let Err(err) =
        agentmux::commands::run_agentmux_mcp_legacy(env::args().skip(1).collect()).await
    {
        agentmux::runtime::inscriptions::emit_inscription(
            "mcp.startup_failed",
            &serde_json::json!({ "error": err.to_string() }),
        );
        eprintln!("agentmux-mcp: {err}");
        std::process::exit(1);
    }
}
