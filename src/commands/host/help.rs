pub(super) fn print_host_help() {
    println!("Usage: agentmux host <relay|mcp> [options]");
}

pub(super) fn print_host_relay_help() {
    println!(
        "Usage: agentmux host relay [--no-autostart] [--require-credentials] [--no-watch] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
    println!();
    println!(
        "  --require-credentials  CLI override for relay.toml require-session-credentials (default false)."
    );
    println!("  --no-watch             CLI override for relay.toml watch-bundles (default true).");
    println!("  Relay-wide controls resolve as: CLI override > environment override");
    println!("  (AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS, AGENTMUX_RELAY_WATCH_BUNDLES) >");
    println!("  <config-root>/relay.toml > defaults.");
}

pub(super) fn print_host_mcp_help() {
    println!(
        "Usage: agentmux host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
