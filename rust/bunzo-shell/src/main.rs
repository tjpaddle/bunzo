use std::env;
use std::fs;
use std::io::{self, BufRead, Stdout, Write};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
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
                "hi, I'm bunzo. this is an M2 stub — type something and hit enter.".into(),
            )],
            input: String::new(),
        }
    }

    fn submit(&mut self) {
        let msg = std::mem::take(&mut self.input);
        let msg = msg.trim().to_string();
        if msg.is_empty() {
            return;
        }
        let reply = format!("(stub) I heard: {msg}");
        self.history.push((Role::User, msg));
        self.history.push((Role::Bunzo, reply));
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
    let mut app = App::new();
    let mut line = String::new();

    writeln!(stdout, "{}", app.banner)?;
    writeln!(stdout)?;
    for (_, text) in &app.history {
        writeln!(stdout, "bunzo {text}")?;
    }
    writeln!(stdout)?;
    stdout.flush()?;

    loop {
        write!(stdout, "> ")?;
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

        writeln!(stdout, "you   {input}")?;
        app.input.clear();
        app.input.push_str(input);
        app.submit();
        if let Some((Role::Bunzo, reply)) = app.history.last() {
            writeln!(stdout, "bunzo {reply}")?;
        }
        writeln!(stdout)?;
        stdout.flush()?;
    }
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
                KeyCode::Enter => app.submit(),
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
