use std::env;

#[tokio::main]
async fn main() {
    if let Err(err) = agentmux::commands::run_agentmux(env::args().skip(1).collect()).await {
        eprintln!("agentmux: {err}");
        std::process::exit(1);
    }
}
