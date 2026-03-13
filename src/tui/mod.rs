//! Interactive terminal workbench for `agentmux` operator workflows.

mod input;
mod render;
mod state;

use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::runtime::error::RuntimeError;

pub use state::{
    TuiLaunchOptions, autocomplete_recipient_input, merge_tui_targets, parse_tui_target_identifier,
};

pub fn run(options: TuiLaunchOptions) -> Result<(), RuntimeError> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, options);
    ratatui::restore();
    result
}

fn run_loop(terminal: &mut DefaultTerminal, options: TuiLaunchOptions) -> Result<(), RuntimeError> {
    let mut state = state::AppState::new(options);
    if let Err(error) = state.refresh_recipients() {
        state.push_runtime_error(error);
    }

    while !state.should_quit {
        terminal
            .draw(|frame| render::render(frame, &mut state))
            .map_err(|source| RuntimeError::io("render tui frame", source))?;

        if !event::poll(Duration::from_millis(80))
            .map_err(|source| RuntimeError::io("poll terminal events", source))?
        {
            continue;
        }

        let event =
            event::read().map_err(|source| RuntimeError::io("read terminal event", source))?;
        if matches!(event, Event::Resize(_, _)) {
            continue;
        }
        if let Err(error) = input::handle_event(&mut state, event) {
            state.push_runtime_error(error);
        }
    }
    Ok(())
}
