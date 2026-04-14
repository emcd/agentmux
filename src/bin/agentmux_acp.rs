use std::{
    io::{self, BufRead, BufReader},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

use agentmux::acp::{AcpStdioClient, PermissionRequest, ReplayEntry};
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
    widgets::{Block, Borders, Paragraph},
};

struct Message {
    role: MessageRole,
    text: String,
}

enum MessageRole {
    User,
    Assistant,
    Thinking,
    ToolCall,
    ToolResult,
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

fn replay_entries_to_messages(entries: Vec<ReplayEntry>) -> Vec<Message> {
    entries
        .into_iter()
        .map(|entry| match entry {
            ReplayEntry::User(lines) => Message {
                role: MessageRole::User,
                text: lines.join("\n"),
            },
            ReplayEntry::Agent(lines) => Message {
                role: MessageRole::Assistant,
                text: lines.join("\n"),
            },
            ReplayEntry::Thinking(lines) => Message {
                role: MessageRole::Thinking,
                text: lines.join("\n"),
            },
            ReplayEntry::ToolCall { title, status } => Message {
                role: MessageRole::ToolCall,
                text: format!("{title} ({status})"),
            },
            ReplayEntry::ToolResult(lines) => Message {
                role: MessageRole::ToolResult,
                text: lines.join("\n"),
            },
        })
        .collect()
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

    let (session_id, initial_messages) = if let Some(id) = session_id {
        let replay_entries = client
            .load_session(&id, &cwd)
            .map_err(|e| anyhow::anyhow!("ACP session/load failed: {e}"))?;
        let msgs = replay_entries_to_messages(replay_entries);
        (id, msgs)
    } else {
        let id = client
            .new_session(&cwd)
            .map_err(|e| anyhow::anyhow!("ACP session/new failed: {e}"))?;
        (id, Vec::new())
    };

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run TUI
    let result = run_tui(&mut terminal, client, &session_id, initial_messages);

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
    initial_messages: Vec<Message>,
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

    for msg in initial_messages {
        app.add_message(msg);
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
                    let mut permission_handler = |req: &PermissionRequest| -> Option<String> {
                        disable_raw_mode().ok();
                        let result = show_permission_menu(req);
                        enable_raw_mode().ok();
                        result
                    };
                    let result = client.prompt(
                        &session,
                        &prompt_text,
                        Some(Duration::from_secs(120)),
                        None,
                        None,
                        Some(&mut permission_handler),
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
    let thinking_label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let tool_label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let system_label_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let user_body_style = Style::default().fg(Color::White);
    let assistant_body_style = Style::default().fg(Color::Rgb(180, 180, 180));
    let thinking_body_style = Style::default()
        .fg(Color::Rgb(120, 120, 120))
        .add_modifier(Modifier::ITALIC);
    let tool_body_style = Style::default().fg(Color::Rgb(180, 180, 100));
    let system_body_style = Style::default().fg(Color::DarkGray);

    let available_width = chunks[1].width.saturating_sub(2) as usize; // subtract block borders

    let history_lines: Vec<Line> = app
        .messages
        .iter()
        .flat_map(|msg| {
            let (label, label_style, body_style) = match msg.role {
                MessageRole::User => ("[User]", user_label_style, user_body_style),
                MessageRole::Assistant => ("[Agent]", assistant_label_style, assistant_body_style),
                MessageRole::Thinking => ("[Cognition]", thinking_label_style, thinking_body_style),
                MessageRole::ToolCall => ("[Invocation]", tool_label_style, tool_body_style),
                MessageRole::ToolResult => ("[Result]", tool_label_style, tool_body_style),
                MessageRole::System => ("[System]", system_label_style, system_body_style),
            };
            let mut lines = Vec::new();
            // Blank line separator before each message
            lines.push(Line::raw(""));
            // Label on its own line
            lines.push(Line::from(Span::styled(label.to_string(), label_style)));
            // Wrap body text to full available width
            for text_line in msg.text.split('\n') {
                for wrapped in wrap_text(text_line, available_width) {
                    lines.push(Line::from(Span::styled(wrapped, body_style)));
                }
            }
            lines
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

    let history =
        Paragraph::new(visible_lines).block(Block::default().borders(Borders::TOP).title(title));
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

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    if text.len() <= width {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= width {
            result.push(remaining.to_string());
            break;
        }
        let wrap_at = if let Some(pos) = remaining[..width].rfind(' ') {
            pos
        } else {
            width
        };
        result.push(remaining[..wrap_at].to_string());
        remaining = remaining[wrap_at..].trim_start();
    }
    result
}

fn show_permission_menu(request: &PermissionRequest) -> Option<String> {
    use std::io::{self, BufRead, Write};
    let stderr = io::stderr();
    let mut out = stderr.lock();
    let _ = writeln!(out, "\n[Permission Required]");
    let _ = writeln!(out, "Tool: {}", request.tool_call_title);
    let _ = writeln!(out);
    for (i, opt) in request.options.iter().enumerate() {
        let _ = writeln!(out, "  [{}] {} ({})", i + 1, opt.name, opt.kind);
    }
    let _ = writeln!(out);
    let _ = write!(out, "Select option (1-{}): ", request.options.len());
    let _ = out.flush();
    let stdin = io::stdin();
    let mut input = String::new();
    if stdin.lock().read_line(&mut input).is_ok()
        && let Ok(idx) = input.trim().parse::<usize>()
        && idx >= 1
        && idx <= request.options.len()
    {
        return Some(request.options[idx - 1].option_id.clone());
    }
    None
}
