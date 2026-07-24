//! Shared command execution for agentmux binaries.

use std::{io::IsTerminal, path::PathBuf};

use crate::runtime::error::RuntimeError;

mod bundle;
mod change;
mod check;
mod down;
mod host;
mod list;
mod look;
mod new;
mod raww;
mod send;
mod shared;
mod tui;
mod up;

#[derive(Clone, Debug, Default)]
pub(super) struct RuntimeArguments {
    pub(super) configuration_root: Option<PathBuf>,
    pub(super) state_root: Option<PathBuf>,
    pub(super) inscriptions_root: Option<PathBuf>,
    pub(super) repository_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(super) struct RelayHostArguments {
    pub(super) no_autostart: bool,
    /// CLI override for `require-session-credentials`. `Some(true)` from
    /// `--require-credentials`; `None` when the flag is absent, leaving the value
    /// to the environment override, `relay.toml`, then the default. Not itself a
    /// default — precedence resolution lives in the relay configuration loader.
    pub(super) require_session_credentials: Option<bool>,
    /// CLI override for `watch-bundles`. `Some(false)` from `--no-watch`; `None`
    /// when the flag is absent, deferring to environment, `relay.toml`, then the
    /// default-on behavior.
    pub(super) watch_bundles: Option<bool>,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) enum BundleAction {
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub(super) enum BundleSelector {
    Bundle(String),
    Group(String),
}

#[derive(Clone, Debug)]
pub(super) struct BundleArguments {
    pub(super) action: BundleAction,
    pub(super) selector: BundleSelector,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug, Default)]
pub(super) struct McpHostArguments {
    pub(super) bundle_name: Option<String>,
    pub(super) session_name: Option<String>,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ListArguments {
    pub(super) namespace: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) output_json: bool,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct LookArguments {
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) target_session: String,
    pub(super) lines: Option<u64>,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct TuiArguments {
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) lines: Option<u64>,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct SendArguments {
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) request_id: Option<String>,
    pub(super) message: String,
    pub(super) targets: Vec<String>,
    pub(super) broadcast: bool,
    pub(super) output_json: bool,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct NewPeerArguments {
    pub(super) principal_id: String,
    pub(super) scope: Option<String>,
    pub(super) output_path: Option<String>,
    pub(super) write_to_config: bool,
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) output_json: bool,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct ChangePskArguments {
    pub(super) principal_id: String,
    pub(super) output_path: Option<String>,
    pub(super) write_to_config: bool,
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) output_json: bool,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CheckArguments {
    /// Optional single bundle to validate; `None` validates every discoverable
    /// bundle.
    pub(super) bundle_id: Option<String>,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct RawwArguments {
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) target_session: String,
    pub(super) text: String,
    pub(super) no_enter: bool,
    pub(super) output_json: bool,
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) struct RelayHostStartupBundle {
    pub(super) bundle_name: String,
    pub(super) outcome: String,
    pub(super) reason_code: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) details: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub(super) struct RelayHostStartupSummary {
    pub(super) schema_version: u32,
    pub(super) host_mode: String,
    pub(super) bundles: Vec<RelayHostStartupBundle>,
    pub(super) hosted_bundle_count: usize,
    pub(super) skipped_bundle_count: usize,
    pub(super) failed_bundle_count: usize,
    pub(super) hosted_any: bool,
}

#[derive(Clone, Debug)]
pub(super) struct BundleTransitionResult {
    pub(super) bundle_name: String,
    pub(super) outcome: String,
    pub(super) reason_code: Option<String>,
    pub(super) reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct BundleTransitionSummary {
    pub(super) schema_version: u32,
    pub(super) action: String,
    pub(super) bundles: Vec<BundleTransitionResult>,
    pub(super) changed_bundle_count: usize,
    pub(super) skipped_bundle_count: usize,
    pub(super) failed_bundle_count: usize,
    pub(super) changed_any: bool,
}

pub(super) const LOOK_LINES_MINIMUM: u64 = 1;
pub(super) const LOOK_LINES_MAXIMUM: u64 = 1000;

/// Runs the unified `agentmux` CLI entrypoint.
pub async fn run_agentmux(arguments: Vec<String>) -> Result<(), RuntimeError> {
    if arguments.is_empty() {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return tui::run_agentmux_tui(&[]);
        }
        print_agentmux_help();
        return Err(RuntimeError::validation(
            "validation_missing_subcommand",
            "no subcommand provided in non-interactive context",
        ));
    }

    match arguments[0].as_str() {
        "--help" | "-h" => {
            print_agentmux_help();
            Ok(())
        }
        "--version" | "-V" => {
            print_agentmux_version();
            Ok(())
        }
        "host" => host::run_agentmux_host(&arguments[1..]).await,
        "up" => up::run_agentmux_up(&arguments[1..]),
        "down" => down::run_agentmux_down(&arguments[1..]),
        "list" => list::run_agentmux_list(&arguments[1..]),
        "look" => look::run_agentmux_look(&arguments[1..]),
        "raww" => raww::run_agentmux_raww(&arguments[1..]),
        "new" => new::run_agentmux_new(&arguments[1..]),
        "change" => change::run_agentmux_change(&arguments[1..]),
        "check" => check::run_agentmux_check(&arguments[1..]),
        "tui" => tui::run_agentmux_tui(&arguments[1..]),
        "send" => send::run_agentmux_send(&arguments[1..]),
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown subcommand".to_string(),
        }),
    }
}

fn print_agentmux_version() {
    println!(concat!(
        env!("CARGO_PKG_NAME"),
        " ",
        env!("CARGO_PKG_VERSION")
    ));
}

fn print_agentmux_help() {
    println!(concat!(
        "Usage: agentmux <command> [options]\n",
        "\n",
        "Commands:\n",
        "  host relay [--no-autostart] [--require-credentials] [--no-watch] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  host mcp [--bundle NAME] [--session-name NAME] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  up (<bundle-id> | --group GROUP) [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  down (<bundle-id> | --group GROUP) [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  list principals [--namespace NAME|GLOBAL|*] [--as-session NAME] [--json] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  look <target-session> [--bundle NAME] [--as-session NAME] [--lines N] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  raww <target-session> --text TEXT [--no-enter] [--bundle NAME] ",
        "[--as-session NAME] [--json] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  new peer <principal_id> [--scope SCOPE] [--output PATH] [--bundle NAME] ",
        "[--as-session NAME] [--json] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  change psk <principal_id> [--bundle NAME] [--as-session NAME] [--json] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  check configuration [<bundle-id>] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "  tui [--bundle NAME] [--as-session NAME] [--lines N] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  send (--target NAME ... | --broadcast) [--message TEXT] ",
        "[--request-id ID] [--bundle NAME] ",
        "[--as-session NAME] [--json] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]\n",
        "\n",
        "Global flags:\n",
        "  -h, --help     Print this help and exit\n",
        "  -V, --version  Print the agentmux version and exit"
    ));
}
