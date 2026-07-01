//! `agentmux-pty` — operator smoke-test entry point for the libghostty-vt binding.
//!
//! This binary exercises the libghostty-vt Rust binding surface used by the
//! Pty transport. It exists so operators can smoke-test Pty sessions
//! end-to-end before configuring a bundle: spawn a child under a
//! portable-pty PTY, pump the child's terminal output through a
//! libghostty-vt `Terminal` on the main thread, render via ratatui, and
//! round-trip keystrokes back through the PTY via `key::Encoder`.
//! Pattern mirrors `agentmux-acp`.
//!
//! Status: operator smoke-test entry point. The relay does not use this
//! binary; the relay constructs its own `PtyTransport` per target via
//! `TransportImpl::pty`. Gated behind the default-off `pty` Cargo
//! feature so default builds do not pull in libghostty-vt /
//! portable-pty and do not invoke Zig. The bin target name uses a
//! hyphen; the Cargo binary is named `agentmux-pty`:
//!
//! ```bash
//! cargo build --features pty --bin agentmux-pty
//! cargo run --features pty --bin agentmux-pty -- /bin/bash
//! ```
//!
//! Thread-safety note: libghostty-vt types are `!Send + !Sync`. The
//! terminal must live on a single thread — here, the main thread. The
//! PTY reader runs on its own thread and forwards raw bytes to the main
//! thread via a channel.

#![forbid(unsafe_code)]

use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use libghostty_vt::{
    Terminal, TerminalOptions,
    fmt::{Format, Formatter, FormatterOptions},
    key::{Action, Encoder as KeyEnc, Event as KeyEventBuf, Key, Mods},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use ratatui::{
    Frame, Terminal as RatatuiTerminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

const COLS: u16 = 120;
const ROWS: u16 = 40;

// Buffer for the terminal's effect-handler-driven PTY writes (device
// attribute responses, size queries, etc.). Effect handlers run
// synchronously inside `vt_write` on the main thread, so a `RefCell` is
// sufficient — the main thread both writes here and drains it.
thread_local! {
    static PTY_RESPONSE_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Wraps the PTY master writer so the main thread can write to it from the
/// input / response-drain paths while the reader thread owns the read side.
struct PtyMaster(Mutex<Box<dyn Write + Send>>);

impl PtyMaster {
    fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
        let mut g = self.0.lock().expect("pty master poisoned");
        g.write_all(data)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (program, program_args, cwd) = parse_args(&args)?;
    eprintln!(
        "agentmux-pty: cols={COLS} rows={ROWS} cmd={program:?} args={program_args:?} cwd={cwd:?}"
    );

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| anyhow!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new(&program);
    for a in &program_args {
        cmd.arg(a);
    }
    if let Some(dir) = &cwd {
        cmd.cwd(dir);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| anyhow!("spawn_command: {e}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| anyhow!("clone_reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| anyhow!("take_writer: {e}"))?;
    let pty_master = Arc::new(PtyMaster(Mutex::new(writer)));

    let mut terminal = Terminal::new(TerminalOptions {
        cols: COLS,
        rows: ROWS,
        max_scrollback: 10_000,
    })
    .map_err(|e| anyhow!("Terminal::new: {e}"))?;

    terminal
        .on_pty_write(|_t, data| {
            PTY_RESPONSE_BUF.with(|buf| buf.borrow_mut().extend_from_slice(data));
        })
        .map_err(|e| anyhow!("on_pty_write: {e}"))?;

    terminal
        .on_size(|_t| {
            Some(libghostty_vt::terminal::SizeReportSize {
                rows: ROWS,
                columns: COLS,
                cell_width: 8,
                cell_height: 16,
            })
        })
        .map_err(|e| anyhow!("on_size: {e}"))?;

    terminal
        .on_device_attributes(|_t| {
            use libghostty_vt::terminal::{
                ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
                PrimaryDeviceAttributes, SecondaryDeviceAttributes,
            };
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[
                        DeviceAttributeFeature::COLUMNS_132,
                        DeviceAttributeFeature::SELECTIVE_ERASE,
                        DeviceAttributeFeature::ANSI_COLOR,
                    ],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 1,
                    rom_cartridge: 0,
                },
                tertiary: Default::default(),
            })
        })
        .map_err(|e| anyhow!("on_device_attributes: {e}"))?;

    terminal
        .on_xtversion(|_t| Some("agentmux-pty (libghostty-vt POC)"))
        .map_err(|e| anyhow!("on_xtversion: {e}"))?;

    terminal
        .on_title_changed(|t| {
            if let Ok(title) = t.title() {
                eprintln!("[title] {title}");
            }
        })
        .map_err(|e| anyhow!("on_title_changed: {e}"))?;

    let (tx_bytes, rx_bytes) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut reader = BufReader::with_capacity(32 * 1024, reader);
        let mut chunk = Vec::with_capacity(32 * 1024);
        loop {
            chunk.clear();
            match reader.read_until(b'\n', &mut chunk) {
                Ok(0) => break,
                Ok(_) => {
                    if tx_bytes.send(chunk.clone()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("[pty reader] {e}");
                    break;
                }
            }
        }
        eprintln!("[pty reader] exited");
    });

    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("EnterAlternateScreen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut ratatui_term = RatatuiTerminal::new(backend).context("ratatui Terminal::new")?;

    let run_result = run_loop(&mut ratatui_term, &mut terminal, rx_bytes, pty_master);

    disable_raw_mode().ok();
    execute!(ratatui_term.backend_mut(), LeaveAlternateScreen).ok();
    ratatui_term.show_cursor().ok();

    let _ = child.wait();
    run_result
}

fn parse_args(args: &[String]) -> Result<(String, Vec<String>, Option<PathBuf>)> {
    let mut program: Option<String> = None;
    let mut program_args: Vec<String> = Vec::new();
    let mut cwd: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "--cwd" => {
                cwd = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                if program.is_none() {
                    program = Some(other.to_string());
                } else {
                    program_args.push(other.to_string());
                }
                i += 1;
            }
        }
    }
    let program = program.unwrap_or_else(|| "/bin/bash".to_string());
    Ok((program, program_args, cwd))
}

fn run_loop(
    ratatui_term: &mut RatatuiTerminal<CrosstermBackend<std::io::Stdout>>,
    terminal: &mut Terminal<'_, '_>,
    rx_bytes: mpsc::Receiver<Vec<u8>>,
    pty_master: Arc<PtyMaster>,
) -> Result<()> {
    let mut enc = KeyEnc::new().map_err(|e| anyhow!("KeyEnc::new: {e}"))?;
    let mut key_event = KeyEventBuf::new().map_err(|e| anyhow!("KeyEventBuf::new: {e}"))?;
    let mut encoded: Vec<u8> = Vec::with_capacity(64);

    loop {
        // 1. Drain PTY bytes -> vt_write, then drain effect-handler responses
        //    back to the PTY.
        let mut any_input = false;
        while let Ok(chunk) = rx_bytes.try_recv() {
            terminal.vt_write(&chunk);
            any_input = true;
        }
        if any_input {
            PTY_RESPONSE_BUF.with(|buf| {
                let mut g = buf.borrow_mut();
                if !g.is_empty() {
                    let _ = pty_master.write_all(&g);
                    g.clear();
                }
            });
        }

        // 2. Poll for keys (non-blocking with timeout).
        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
            && handle_key(
                key,
                terminal,
                &mut enc,
                &mut key_event,
                &mut encoded,
                &pty_master,
            )?
        {
            break;
        }

        // 3. Render — formatter borrows terminal immutably, so vt_write above
        //    must not overlap this draw call. The closure is dropped after
        //    draw() returns.
        ratatui_term.draw(|frame| {
            render_screen(frame, terminal);
        })?;

        // 4. Drain any final bytes that arrived between render and now.
        while let Ok(chunk) = rx_bytes.try_recv() {
            terminal.vt_write(&chunk);
        }
    }
    Ok(())
}

fn handle_key(
    key: KeyEvent,
    terminal: &mut Terminal<'_, '_>,
    enc: &mut KeyEnc<'_>,
    key_event: &mut KeyEventBuf<'_>,
    encoded: &mut Vec<u8>,
    pty_master: &Arc<PtyMaster>,
) -> Result<bool> {
    let mods = crossterm_mods_to_libghostty(key.modifiers);
    let (lg_key, ucp, utf8_text) = crossterm_key_to_libghostty(key.code);
    encoded.clear();

    key_event
        .set_action(Action::Press)
        .set_key(lg_key)
        .set_mods(mods)
        .set_consumed_mods(Mods::empty())
        .set_unshifted_codepoint(ucp)
        .set_utf8(utf8_text);

    enc.set_options_from_terminal(terminal);
    enc.encode_to_vec(key_event, encoded)
        .map_err(|e| anyhow!("key encode: {e}"))?;

    if !encoded.is_empty() {
        pty_master
            .write_all(encoded)
            .map_err(|e| anyhow!("pty write: {e}"))?;
    }
    Ok(key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
}

fn crossterm_mods_to_libghostty(m: KeyModifiers) -> Mods {
    let mut out = Mods::empty();
    if m.contains(KeyModifiers::SHIFT) {
        out |= Mods::SHIFT;
    }
    if m.contains(KeyModifiers::ALT) {
        out |= Mods::ALT;
    }
    if m.contains(KeyModifiers::CONTROL) {
        out |= Mods::CTRL;
    }
    if m.contains(KeyModifiers::SUPER) {
        out |= Mods::SUPER;
    }
    out
}

fn crossterm_key_to_libghostty(code: KeyCode) -> (Key, char, Option<String>) {
    use Key::*;
    match code {
        KeyCode::Char(c) => (Unidentified, c, Some(c.to_string())),
        KeyCode::Enter => (Enter, '\r', None),
        KeyCode::Backspace => (Backspace, '\u{0008}', None),
        KeyCode::Tab => (Tab, '\t', None),
        KeyCode::Esc => (Escape, '\u{001b}', None),
        KeyCode::Up => (ArrowUp, '\0', None),
        KeyCode::Down => (ArrowDown, '\0', None),
        KeyCode::Left => (ArrowLeft, '\0', None),
        KeyCode::Right => (ArrowRight, '\0', None),
        KeyCode::Home => (Home, '\0', None),
        KeyCode::End => (End, '\0', None),
        KeyCode::PageUp => (PageUp, '\0', None),
        KeyCode::PageDown => (PageDown, '\0', None),
        KeyCode::Insert => (Insert, '\0', None),
        KeyCode::Delete => (Delete, '\u{007f}', None),
        KeyCode::F(n) => f_key(n),
        _ => (Unidentified, '\0', None),
    }
}

fn f_key(n: u8) -> (Key, char, Option<String>) {
    use Key::*;
    let k = match n {
        1 => F1,
        2 => F2,
        3 => F3,
        4 => F4,
        5 => F5,
        6 => F6,
        7 => F7,
        8 => F8,
        9 => F9,
        10 => F10,
        11 => F11,
        12 => F12,
        _ => Unidentified,
    };
    (k, '\0', None)
}

fn render_screen(frame: &mut Frame<'_>, terminal: &Terminal<'_, '_>) {
    // Formatter is created and dropped within this function so the &Terminal
    // borrow it holds is released before any later &mut calls.
    let mut formatter = match Formatter::new(
        terminal,
        FormatterOptions::new()
            .with_format(Format::Plain)
            .with_unwrap(true)
            .with_trim(true),
    ) {
        Ok(f) => f,
        Err(e) => {
            frame.render_widget(
                Paragraph::new(format!("formatter init failed: {e}"))
                    .block(Block::default().borders(Borders::ALL)),
                frame.area(),
            );
            return;
        }
    };

    let bytes = match formatter.format_alloc(None) {
        Ok(b) => b,
        Err(e) => {
            frame.render_widget(
                Paragraph::new(format!("format_alloc failed: {e}"))
                    .block(Block::default().borders(Borders::ALL)),
                frame.area(),
            );
            return;
        }
    };
    let text = std::str::from_utf8(&bytes).unwrap_or("<non-utf8>");

    let lines: Vec<Line<'_>> = text
        .split('\n')
        .take(ROWS as usize)
        .map(|row| {
            Line::from(Span::styled(
                row.to_string(),
                Style::default().fg(Color::White),
            ))
        })
        .collect();

    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" agentmux-pty (libghostty-vt POC) — Ctrl+C to quit ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut all_lines = lines;
    while all_lines.len() < ROWS as usize {
        all_lines.push(Line::from(""));
    }

    let para = Paragraph::new(all_lines);
    frame.render_widget(para, inner);

    let cursor_x = terminal.cursor_x().unwrap_or(0);
    let cursor_y = terminal.cursor_y().unwrap_or(0);
    if cursor_x < inner.width && cursor_y < inner.height {
        let cursor_block = Rect::new(inner.x + cursor_x, inner.y + cursor_y, 1, 1);
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(Color::Gray)),
            cursor_block,
        );
    }
}
