use std::env;
use std::fs;
use std::io::{self, BufRead, Stdout, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bunzo_proto::{
    read_frame, write_frame, ClientMessage, Envelope, ServerFrame, ServerMessage,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    style::Stylize,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal, TerminalOptions, Viewport,
};

type Tui = Terminal<CrosstermBackend<Stdout>>;
const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_LINES: u16 = 24;
const MIN_COLUMNS: u16 = 40;
const MIN_LINES: u16 = 10;
const BUNZOD_SOCKET: &str = "/run/bunzod.sock";

struct App {
    banner: String,
    history: Vec<(Role, String)>,
    input: String,
}

enum Role {
    User,
    Bunzo,
}

impl App {
    fn new() -> Self {
        Self {
            banner: read_banner(),
            history: vec![(
                Role::Bunzo,
                "hi, I'm bunzo. type something and hit enter.".into(),
            )],
            input: String::new(),
        }
    }
}

fn read_banner() -> String {
    fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "bunzo".into())
}

fn main() -> io::Result<()> {
    if shell_mode() == "serial" {
        return run_serial_shell();
    }

    let mut terminal = setup_terminal()?;
    let mut app = App::new();
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn shell_mode() -> String {
    env::var("BUNZO_SHELL_MODE").unwrap_or_else(|_| "serial".into())
}

fn run_serial_shell() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();
    let banner = read_banner();
    let mut line = String::new();
    let mut msg_counter: u64 = 0;

    write!(stdout, "\x1B[2J\x1B[H")?;
    writeln!(stdout, "{}", banner.as_str().bold().cyan())?;
    writeln!(stdout, "{}", "─".repeat(60).as_str().dark_grey())?;
    writeln!(
        stdout,
        "{} connected — type to talk to bunzod.",
        "bunzo".bold().magenta(),
    )?;
    writeln!(stdout)?;
    stdout.flush()?;

    loop {
        write!(stdout, "{} ", ">".cyan().bold())?;
        stdout.flush()?;

        line.clear();
        let bytes = stdin.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(());
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if matches!(input, "exit" | "quit" | ":q") {
            return Ok(());
        }

        msg_counter = msg_counter.wrapping_add(1);
        let id = format!("u{msg_counter}");

        // Print the reply tag before the stream so chunks land right after it.
        write!(stdout, "{} ", "bunzo".bold().magenta())?;
        stdout.flush()?;

        match round_trip(&id, input, &mut stdout) {
            Ok(()) => {}
            Err(RoundTripError::Unreachable(reason)) => {
                writeln!(
                    stdout,
                    "{}",
                    format!("[bunzod unreachable: {reason}]").red()
                )?;
            }
            Err(RoundTripError::Protocol(reason)) => {
                writeln!(stdout, "{}", format!("[protocol error: {reason}]").red())?;
            }
            Err(RoundTripError::Remote { code, text }) => {
                writeln!(stdout, "{}", format!("[{code}] {text}").red())?;
            }
        }
        writeln!(stdout)?;
        stdout.flush()?;
    }
}

enum RoundTripError {
    Unreachable(String),
    Protocol(String),
    Remote { code: String, text: String },
}

fn round_trip(id: &str, text: &str, stdout: &mut Stdout) -> Result<(), RoundTripError> {
    let mut stream =
        UnixStream::connect(BUNZOD_SOCKET).map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    // A misbehaving daemon shouldn't hang the shell forever. Generous per-op
    // timeout; still tight enough that a hung daemon gets caught quickly.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::UserMessage {
        id: id.into(),
        text: text.into(),
    });
    write_frame(&mut stream, &req).map_err(|e| RoundTripError::Protocol(e.to_string()))?;

    loop {
        let frame: ServerFrame = match read_frame(&mut stream) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(RoundTripError::Protocol(
                    "bunzod closed the connection mid-stream".into(),
                ));
            }
            Err(e) => return Err(RoundTripError::Protocol(e.to_string())),
        };

        match frame.msg {
            ServerMessage::AssistantChunk {
                id: chunk_id,
                text,
            } if chunk_id == id => {
                write!(stdout, "{text}").map_err(|e| RoundTripError::Protocol(e.to_string()))?;
                stdout.flush().map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::AssistantChunk { .. } => {
                // Out-of-turn chunk: ignore rather than fail.
            }
            ServerMessage::AssistantEnd { id: end_id, .. } if end_id == id => {
                return Ok(());
            }
            ServerMessage::AssistantEnd { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            ServerMessage::ToolActivity {
                id: act_id,
                name,
                phase,
                detail,
            } if act_id == id => {
                render_tool_activity(stdout, &name, &phase, &detail)
                    .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::ToolActivity { .. } => {}
        }
    }
}

fn render_tool_activity(
    stdout: &mut Stdout,
    name: &str,
    phase: &str,
    detail: &str,
) -> io::Result<()> {
    // Break out of the assistant's in-flight text so the status sits on its
    // own line, then return to the assistant tag so subsequent chunks keep
    // streaming where they were.
    writeln!(stdout)?;
    let line = match phase {
        "invoke" => format!("→ {name} …").dark_grey().italic().to_string(),
        "ok" => format!("✓ {name}").dark_grey().italic().to_string(),
        "error" => {
            let suffix = if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            };
            format!("✗ {name}{suffix}").red().italic().to_string()
        }
        other => format!("· {name} ({other})").dark_grey().italic().to_string(),
    };
    writeln!(stdout, "{line}")?;
    write!(stdout, "{} ", "bunzo".bold().magenta())?;
    stdout.flush()
}

fn setup_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // No EnterAlternateScreen / EnableMouseCapture: bunzo's primary console is a
    // PL011 UART (ttyAMA0), which is a dumb serial line — neither escape works
    // there. Clear the screen instead so the TUI starts on a fresh canvas.
    execute!(stdout, Clear(ClearType::All))?;
    let (columns, lines) = serial_viewport();
    Terminal::with_options(
        CrosstermBackend::new(stdout),
        TerminalOptions {
            // Serial consoles often report a 0x0 geometry. Use a fixed viewport
            // so ratatui does not depend on backend-reported terminal size.
            viewport: Viewport::Fixed(Rect::new(0, 0, columns, lines)),
        },
    )
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), Clear(ClearType::All))?;
    terminal.show_cursor()
}

fn run(terminal: &mut Tui, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::Enter => {
                    // TUI path is behind BUNZO_SHELL_MODE=tui and deferred to a
                    // later milestone; keep the echo stub here so the code
                    // path still compiles. Real wiring happens in serial mode.
                    let msg = std::mem::take(&mut app.input);
                    let msg = msg.trim().to_string();
                    if !msg.is_empty() {
                        app.history.push((Role::User, msg.clone()));
                        app.history
                            .push((Role::Bunzo, format!("(tui stub) {msg}")));
                    }
                }
                KeyCode::Backspace => {
                    app.input.pop();
                }
                KeyCode::Char(c) => app.input.push(c),
                _ => {}
            }
        }
    }
}

fn serial_viewport() -> (u16, u16) {
    let columns = env_dimension("COLUMNS", DEFAULT_COLUMNS, MIN_COLUMNS);
    let lines = env_dimension("LINES", DEFAULT_LINES, MIN_LINES);
    (columns, lines)
}

fn env_dimension(name: &str, default: u16, min: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .map(|value| value.max(min))
        .unwrap_or(default)
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    let header = Paragraph::new(Line::from(vec![Span::styled(
        &app.banner,
        Style::default().add_modifier(Modifier::BOLD),
    )]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let lines: Vec<Line> = app
        .history
        .iter()
        .map(|(role, text)| {
            let tag = match role {
                Role::User => Span::styled("you  ", Style::default().add_modifier(Modifier::BOLD)),
                Role::Bunzo => Span::styled("bunzo", Style::default().add_modifier(Modifier::BOLD)),
            };
            Line::from(vec![tag, Span::raw(" "), Span::raw(text.clone())])
        })
        .collect();
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(" chat "));
    f.render_widget(body, chunks[1]);

    let input = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" > "));
    f.render_widget(input, chunks[2]);
}
