pub(super) fn print_host_help() {
    println!("Usage: agentmux host <relay|mcp> [options]");
}

pub(super) fn print_host_relay_help() {
    println!(
        "Usage: agentmux host relay [--no-autostart] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

pub(super) fn print_host_mcp_help() {
    println!(
        "Usage: agentmux host mcp [--bundle NAME] [--session-name NAME] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
