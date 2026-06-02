use std::env;
use std::time::Duration;

// Last-resort cap on how long the relay host runtime waits for in-flight
// blocking tasks (the `spawn_blocking` transport bodies and the ACP
// single-flight prompt wait) to finish at shutdown before abandoning them.
// Graceful shutdown normally drains workers well under this; the bound only
// covers pathological stuck blocking work so a relay SIGTERM cannot hang until
// systemd's SIGKILL. Scoped to the relay host: every other subcommand keeps the
// default runtime-drop teardown.
const RELAY_HOST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    // Only the relay host owns long-lived blocking delivery work whose teardown
    // must be bounded. Detect it from the `host relay` subcommand so CLI, MCP,
    // and TUI invocations are left on default `#[tokio::main]` semantics.
    let is_relay_host = matches!(
        (arguments.first(), arguments.get(1)),
        (Some(first), Some(second)) if first == "host" && second == "relay"
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(source) => {
            eprintln!("agentmux: failed to build async runtime: {source}");
            std::process::exit(1);
        }
    };

    let exit_code = runtime.block_on(async move {
        match agentmux::commands::run_agentmux(arguments).await {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("agentmux: {err}");
                1
            }
        }
    });

    if is_relay_host {
        // Bound the wait for in-flight blocking tasks instead of the unbounded
        // wait an implicit runtime drop performs.
        runtime.shutdown_timeout(RELAY_HOST_SHUTDOWN_TIMEOUT);
    } else {
        // Default teardown for CLI / MCP / TUI: dropping the runtime waits for
        // outstanding blocking tasks, matching prior `#[tokio::main]` behavior.
        drop(runtime);
    }
    std::process::exit(exit_code);
}
