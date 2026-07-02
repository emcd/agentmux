//! Interactive terminal workbench for `agentmux` operator workflows.

mod input;
mod render;
mod state;
mod status;
mod target;
pub mod workbench;

use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::runtime::{
    error::RuntimeError,
    signals::{install_shutdown_signal_handlers, shutdown_requested},
};

pub use state::TuiLaunchOptions;
pub use status::{
    BundleStatusDisplay, BundleStatusSeverity, RecipientReadiness, StartupFailureSummary,
    bundle_status_severity, format_bundle_status_line, format_recipient_picker_label,
    format_startup_failure_lines,
};
pub use target::{
    autocomplete_recipient_input, merge_tui_targets, parse_tui_target_identifier,
    sender_bound_bundle,
};

struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn run(options: TuiLaunchOptions) -> Result<(), RuntimeError> {
    let mut terminal = ratatui::init();
    let _restore_guard = TerminalRestoreGuard;
    let _signal_handlers = install_shutdown_signal_handlers()?;
    run_loop(&mut terminal, options)
}

fn run_loop(terminal: &mut DefaultTerminal, options: TuiLaunchOptions) -> Result<(), RuntimeError> {
    let mut state = state::AppState::new(options);
    if let Err(error) = state.refresh_recipients() {
        state.push_runtime_error(error);
    }
    let mut needs_redraw = true;

    while !state.should_quit && !shutdown_requested() {
        if needs_redraw {
            terminal
                .draw(|frame| render::render(frame, &mut state))
                .map_err(|source| RuntimeError::io("render tui frame", source))?;
            needs_redraw = false;
        }

        // Drain all buffered terminal input before polling the relay. A relay
        // poll can stall (e.g. the hello-ack timeout when the relay is
        // unresponsive); handling input first keeps keystrokes from queueing
        // behind that stall.
        let mut handled_input = false;
        while event::poll(Duration::ZERO)
            .map_err(|source| RuntimeError::io("poll terminal events", source))?
        {
            let event =
                event::read().map_err(|source| RuntimeError::io("read terminal event", source))?;
            handled_input = true;
            needs_redraw = true;
            if matches!(event, Event::Resize(_, _)) {
                continue;
            }
            if let Err(error) = input::handle_event(&mut state, event) {
                state.push_runtime_error(error);
            }
        }
        if handled_input {
            continue;
        }

        if state.poll_relay_events() {
            needs_redraw = true;
            continue;
        }

        event::poll(Duration::from_millis(80))
            .map_err(|source| RuntimeError::io("poll terminal events", source))?;
    }
    Ok(())
}
