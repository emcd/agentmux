use std::{
    io::{self, BufRead, BufReader},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

use agentmux::acp::AcpStdioClient;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

struct Message {
    role: MessageRole,
    text: String,
}

enum MessageRole {
    User,
    Assistant,
    System,
}

enum AppEvent {
    Message(Message),
    PromptComplete(String),
    Error(String),
}

struct App {
    messages: Vec<Message>,
    input: String,
    session_id: String,
    status: String,
    prompt_active: bool,
    should_quit: bool,
    scroll_offset: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    input_draft: String,
}

impl App {
    fn new(session_id: String) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            session_id,
            status: "Ready".to_string(),
            prompt_active: false,
            should_quit: false,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            input_draft: String::new(),
        }
    }

    fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    fn send_prompt(&mut self) {
        let input = self.input.trim().to_string();
        if input.is_empty() || self.prompt_active {
            return;
        }
        self.history.push(input.clone());
        self.history_index = None;
        self.input_draft.clear();
        self.add_message(Message {
            role: MessageRole::User,
            text: input,
        });
        self.input.clear();
        self.prompt_active = true;
        self.scroll_offset = 0;
        self.status = "Processing...".to_string();
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Message(msg) => self.add_message(msg),
            AppEvent::PromptComplete(stop_reason) => {
                self.prompt_active = false;
                self.status = format!("Ready (last: {stop_reason})");
            }
            AppEvent::Error(err) => {
                self.prompt_active = false;
                self.add_message(Message {
                    role: MessageRole::System,
                    text: format!("Error: {err}"),
                });
                self.status = "Error".to_string();
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut command = None;
    let mut session_id = None;
    let mut working_directory = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session-id" => session_id = args.next(),
            "-C" | "--cd" | "--current-directory" => working_directory = args.next(),
            _ if arg.starts_with('-') => {
                eprintln!("unknown option: {arg}");
                std::process::exit(1);
            }
            _ => command = Some(arg),
        }
    }

    let command = command.unwrap_or_else(|| "opencode acp".to_string());
    let cwd = match working_directory {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir()?,
    };

    // Spawn ACP agent and initialize
    let mut client = AcpStdioClient::spawn(&command, &cwd, &[])
        .map_err(|e| anyhow::anyhow!("Failed to spawn ACP agent: {e}"))?;

    let _init_result = client
        .initialize()
        .map_err(|e| anyhow::anyhow!("ACP initialize failed: {e}"))?;

    let session_id = if let Some(id) = session_id {
        client
            .load_session(&id, &cwd)
            .map_err(|e| anyhow::anyhow!("ACP session/load failed: {e}"))?;
        id
    } else {
        client
            .new_session(&cwd)
            .map_err(|e| anyhow::anyhow!("ACP session/new failed: {e}"))?
    };

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run TUI
    let result = run_tui(&mut terminal, client, &session_id);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Drain any key events buffered in crossterm's internal queue.
/// Called after a synchronous prompt to prevent queued keypresses
/// (including Enter) from firing on the next event loop iteration.
fn drain_event_buffer() {
    while let Ok(true) = event::poll(Duration::ZERO) {
        if event::read().is_err() {
            break;
        }
    }
}

fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut client: AcpStdioClient,
    session_id: &str,
) -> anyhow::Result<()> {
    let mut app = App::new(session_id.to_string());

    let (tx, rx) = mpsc::channel::<AppEvent>();

    // Spawn stderr reader for ACP agent diagnostics
    if let Some(stderr) = client.child_stderr() {
        let tx_stderr = tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.trim().is_empty() => {
                        let _ = tx_stderr.send(AppEvent::Message(Message {
                            role: MessageRole::System,
                            text: format!("stderr: {line}"),
                        }));
                    }
                    _ => break,
                }
            }
        });
    }

    app.add_message(Message {
        role: MessageRole::System,
        text: format!("Connected. Session: {session_id}"),
    });

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                break;
            }
            if app.prompt_active {
                continue;
            }
            match key.code {
                KeyCode::Enter => {
                    let prompt_text = app.input.clone();
                    app.send_prompt();
                    if !app.prompt_active {
                        continue;
                    }
                    // Handle prompt synchronously for MVP
                    // (TUI freezes during prompt — acceptable for debugging use case)
                    let session = session_id.to_string();
                    let result = client.prompt(
                        &session,
                        &prompt_text,
                        Some(Duration::from_secs(120)),
                        None,
                        None,
                    );
                    match result {
                        Ok(completion) => {
                            let snapshot = client.take_snapshot_lines();
                            if !snapshot.is_empty() {
                                let _ = tx.send(AppEvent::Message(Message {
                                    role: MessageRole::Assistant,
                                    text: snapshot.join("\n"),
                                }));
                            }
                            let _ = tx.send(AppEvent::PromptComplete(completion.stop_reason));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Error(format!("{e:?}")));
                        }
                    }
                    // Drain events accumulated during prompt
                    while let Ok(event) = rx.try_recv() {
                        app.handle_event(event);
                    }
                    // Drain crossterm input buffer so queued keys
                    // (including a second Enter) don't fire immediately
                    drain_event_buffer();
                    continue;
                }
                KeyCode::Backspace => {
                    app.input.pop();
                    app.history_index = None;
                }
                KeyCode::Char(c) => {
                    app.input.push(c);
                    app.history_index = None;
                }
                KeyCode::PageUp => {
                    app.scroll_offset = app.scroll_offset.saturating_add(10);
                }
                KeyCode::PageDown => {
                    app.scroll_offset = app.scroll_offset.saturating_sub(10);
                }
                KeyCode::Up => {
                    if !app.history.is_empty() {
                        let new_index = match app.history_index {
                            None => app.history.len() - 1,
                            Some(idx) => idx.saturating_sub(1),
                        };
                        if app.history_index.is_none() {
                            app.input_draft = app.input.clone();
                        }
                        app.history_index = Some(new_index);
                        app.input = app.history[new_index].clone();
                    }
                }
                KeyCode::Down => match app.history_index {
                    Some(idx) if idx + 1 < app.history.len() => {
                        let new_index = idx + 1;
                        app.history_index = Some(new_index);
                        app.input = app.history[new_index].clone();
                    }
                    Some(_) => {
                        app.history_index = None;
                        app.input = app.input_draft.clone();
                    }
                    None => {}
                },
                _ => {}
            }
        }

        // Process any accumulated events
        while let Ok(event) = rx.try_recv() {
            app.handle_event(event);
        }

        if app.should_quit {
            break;
        }
    }

    client.kill();
    Ok(())
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(frame.area());

    // Status bar
    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(
                " Session: {} ",
                &app.session_id[..std::cmp::min(12, app.session_id.len())]
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("Status: {} ", app.status),
            Style::default().fg(Color::Green),
        ),
        if app.prompt_active {
            Span::styled(" [busy]", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]));
    frame.render_widget(status, chunks[0]);

    // Conversation history
    let user_label_style = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD);
    let assistant_label_style = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let system_label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let user_body_style = Style::default().fg(Color::White);
    let assistant_body_style = Style::default().fg(Color::Rgb(180, 180, 180));
    let system_body_style = Style::default().fg(Color::DarkGray);

    let history_lines: Vec<Line> = app
        .messages
        .iter()
        .flat_map(|msg| {
            let (label_style, body_style) = match msg.role {
                MessageRole::User => (user_label_style, user_body_style),
                MessageRole::Assistant => (assistant_label_style, assistant_body_style),
                MessageRole::System => (system_label_style, system_body_style),
            };
            msg.text.split('\n').enumerate().map(move |(i, line)| {
                if i == 0 {
                    Line::from(vec![
                        Span::styled(" you  ", label_style),
                        Span::styled(line.to_string(), body_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("      ", label_style),
                        Span::styled(line.to_string(), body_style),
                    ])
                }
            })
        })
        .collect();

    let viewport = (chunks[1].height as usize).saturating_sub(2);
    let end = if app.scroll_offset > 0 {
        history_lines.len().saturating_sub(app.scroll_offset)
    } else {
        history_lines.len()
    };
    let start = end.saturating_sub(viewport);
    let visible_lines: Vec<Line> = history_lines[start..end].to_vec();

    let title = if app.scroll_offset > 0 {
        format!(
            " Scrollback ({} lines up) — PgDn to bottom ",
            app.scroll_offset
        )
    } else {
        " History (PgUp/PgDn) ".to_string()
    };

    let history = Paragraph::new(visible_lines)
        .block(Block::default().borders(Borders::TOP).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(history, chunks[1]);

    // Input area
    let footer = if app.input.is_empty() {
        " type a message and press Enter | Up/Down: history | Ctrl+C: quit "
    } else {
        " Enter: send | Ctrl+C: quit "
    };
    let input_text = format!("> {} ", app.input);
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::TOP).title(footer))
        .style(Style::default().fg(Color::White));
    frame.render_widget(input, chunks[2]);
}
