//! Shared command execution for agentmux binaries.

use std::{io::IsTerminal, path::PathBuf};

use crate::{relay::ChatDeliveryMode, runtime::error::RuntimeError};

mod down;
mod host;
mod lifecycle;
mod list;
mod look;
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
    pub(super) runtime: RuntimeArguments,
}

#[derive(Clone, Debug)]
pub(super) enum LifecycleAction {
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub(super) enum LifecycleSelector {
    Bundle(String),
    Group(String),
}

#[derive(Clone, Debug)]
pub(super) struct LifecycleArguments {
    pub(super) action: LifecycleAction,
    pub(super) selector: LifecycleSelector,
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
    pub(super) bundle_name: Option<String>,
    pub(super) session_selector: Option<String>,
    pub(super) all_bundles: bool,
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
    pub(super) delivery_mode: ChatDeliveryMode,
    pub(super) quiescence_timeout_ms: Option<u64>,
    pub(super) acp_turn_timeout_ms: Option<u64>,
    pub(super) output_json: bool,
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
pub(super) struct LifecycleTransitionBundle {
    pub(super) bundle_name: String,
    pub(super) outcome: String,
    pub(super) reason_code: Option<String>,
    pub(super) reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct LifecycleTransitionSummary {
    pub(super) schema_version: u32,
    pub(super) action: String,
    pub(super) bundles: Vec<LifecycleTransitionBundle>,
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
        "host" => host::run_agentmux_host(&arguments[1..]).await,
        "up" => up::run_agentmux_up(&arguments[1..]),
        "down" => down::run_agentmux_down(&arguments[1..]),
        "list" => list::run_agentmux_list(&arguments[1..]),
        "look" => look::run_agentmux_look(&arguments[1..]),
        "raww" => raww::run_agentmux_raww(&arguments[1..]),
        "tui" => tui::run_agentmux_tui(&arguments[1..]),
        "send" => send::run_agentmux_send(&arguments[1..]),
        unknown => Err(RuntimeError::InvalidArgument {
            argument: unknown.to_string(),
            message: "unknown subcommand".to_string(),
        }),
    }
}

fn print_agentmux_help() {
    println!(concat!(
        "Usage: agentmux <command> [options]\n",
        "\n",
        "Commands:\n",
        "  host relay [--no-autostart] [--config-directory PATH] ",
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
        "  list sessions [--bundle NAME|--all] [--as-session NAME] [--json] ",
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
        "  tui [--bundle NAME] [--as-session NAME] [--lines N] ",
        "[--config-directory PATH] [--state-directory PATH] ",
        "[--inscriptions-directory PATH|--logs-directory PATH] ",
        "[--repository-root PATH]\n",
        "  send (--target NAME ... | --broadcast) [--message TEXT] ",
        "[--delivery-mode async|sync] [--quiescence-timeout-ms MS] ",
        "[--acp-turn-timeout-ms MS] [--request-id ID] [--bundle NAME] ",
        "[--as-session NAME] [--json] [--config-directory PATH] ",
        "[--state-directory PATH] [--inscriptions-directory PATH|",
        "--logs-directory PATH] [--repository-root PATH]"
    ));
}
