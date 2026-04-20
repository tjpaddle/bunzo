use std::env;
use std::fs;
use std::io::{self, BufRead, Stdout, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use bunzo_proto::{
    read_frame, write_frame, ClientMessage, ConversationSummary, Envelope, ServerFrame,
    ServerMessage, TaskSummary,
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
const BUNZOD_CONFIG_DIR: &str = "/etc/bunzo";
const BUNZOD_CONFIG_PATH: &str = "/etc/bunzo/bunzod.toml";
const OPENAI_KEY_PATH: &str = "/etc/bunzo/openai.key";
const DEFAULT_REMOTE_MODEL: &str = "gpt-5.4-mini";
const RECENT_CONVERSATION_LIMIT: u32 = 12;
const RECENT_TASK_LIMIT: u32 = 16;

struct App {
    banner: String,
    history: Vec<(Role, String)>,
    input: String,
}

enum Role {
    User,
    Bunzo,
}

#[derive(Default)]
struct ShellState {
    active_conversation: Option<String>,
}

struct RoundTripOutcome {
    conversation_id: String,
    created_conversation: bool,
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
    let mut shell_state = ShellState::default();

    write!(stdout, "\x1B[2J\x1B[H")?;
    writeln!(stdout, "{}", banner.as_str().bold().cyan())?;
    writeln!(stdout, "{}", "─".repeat(60).as_str().dark_grey())?;
    writeln!(
        stdout,
        "{} connected — type to talk to bunzod.",
        "bunzo".bold().magenta(),
    )?;
    if let Some(issue) = local_setup_issue() {
        writeln!(
            stdout,
            "{}",
            format!(
                "setup needed — {} Type /setup to paste your API key.",
                issue
            )
            .yellow()
        )?;
    }
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
        if matches!(input, "/setup" | ":setup") {
            let _ = run_openai_setup(&mut stdin, &mut stdout, None)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }
        if let Some(args) = command_args(input, &["/conversations", "/conv"]) {
            handle_conversations_command(args, &mut shell_state, &mut msg_counter, &mut stdout)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }
        if let Some(args) = command_args(input, &["/tasks"]) {
            handle_tasks_command(args, &mut msg_counter, &mut stdout)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }

        if let Some(issue) = local_setup_issue() {
            if !run_openai_setup(&mut stdin, &mut stdout, Some(&issue))? {
                writeln!(stdout)?;
                stdout.flush()?;
                continue;
            }
        }

        msg_counter = msg_counter.wrapping_add(1);
        let id = format!("u{msg_counter}");

        // Print the reply tag before the stream so chunks land right after it.
        write!(stdout, "{} ", "bunzo".bold().magenta())?;
        stdout.flush()?;

        let requested_conversation = shell_state.active_conversation.as_deref();
        match round_trip(&id, requested_conversation, input, &mut stdout) {
            Ok(outcome) => {
                if shell_state.active_conversation.is_some() {
                    shell_state.active_conversation = Some(outcome.conversation_id);
                } else if outcome.created_conversation {
                    writeln!(
                        stdout,
                        "{}",
                        format!(
                            "[saved as {} — use /conversations {} to resume]",
                            short_id(&outcome.conversation_id),
                            short_id(&outcome.conversation_id)
                        )
                        .dark_grey()
                    )?;
                }
            }
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
                if should_offer_setup(&code, &text)
                    && run_openai_setup(&mut stdin, &mut stdout, Some(&text))?
                {
                    msg_counter = msg_counter.wrapping_add(1);
                    let retry_id = format!("u{msg_counter}");
                    write!(stdout, "{} ", "bunzo".bold().magenta())?;
                    stdout.flush()?;
                    let requested_conversation = shell_state.active_conversation.as_deref();
                    match round_trip(&retry_id, requested_conversation, input, &mut stdout) {
                        Ok(outcome) => {
                            if shell_state.active_conversation.is_some() {
                                shell_state.active_conversation = Some(outcome.conversation_id);
                            } else if outcome.created_conversation {
                                writeln!(
                                    stdout,
                                    "{}",
                                    format!(
                                        "[saved as {} — use /conversations {} to resume]",
                                        short_id(&outcome.conversation_id),
                                        short_id(&outcome.conversation_id)
                                    )
                                    .dark_grey()
                                )?;
                            }
                        }
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
                } else {
                    writeln!(stdout, "{}", format!("[{code}] {text}").red())?;
                }
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

fn round_trip(
    id: &str,
    conversation_id: Option<&str>,
    text: &str,
    stdout: &mut Stdout,
) -> Result<RoundTripOutcome, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    // A misbehaving daemon shouldn't hang the shell forever. Generous per-op
    // timeout; still tight enough that a hung daemon gets caught quickly.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::UserMessage {
        id: id.into(),
        text: text.into(),
        conversation_id: conversation_id.map(str::to_string),
    });
    write_frame(&mut stream, &req).map_err(|e| RoundTripError::Protocol(e.to_string()))?;

    let mut outcome: Option<RoundTripOutcome> = None;

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
            ServerMessage::RequestContext {
                id: ctx_id,
                conversation_id,
                created_conversation,
                ..
            } if ctx_id == id => {
                outcome = Some(RoundTripOutcome {
                    conversation_id,
                    created_conversation,
                });
            }
            ServerMessage::RequestContext { .. } => {}
            ServerMessage::AssistantChunk { id: chunk_id, text } if chunk_id == id => {
                write!(stdout, "{text}").map_err(|e| RoundTripError::Protocol(e.to_string()))?;
                stdout
                    .flush()
                    .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::AssistantChunk { .. } => {
                // Out-of-turn chunk: ignore rather than fail.
            }
            ServerMessage::AssistantEnd { id: end_id, .. } if end_id == id => {
                return outcome.ok_or_else(|| {
                    RoundTripError::Protocol(
                        "bunzod ended the request without request context".into(),
                    )
                });
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
            ServerMessage::ConversationList { .. } => {}
            ServerMessage::TaskList { .. } => {}
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
        other => format!("· {name} ({other})")
            .dark_grey()
            .italic()
            .to_string(),
    };
    writeln!(stdout, "{line}")?;
    write!(stdout, "{} ", "bunzo".bold().magenta())?;
    stdout.flush()
}

fn handle_conversations_command(
    args: &str,
    shell_state: &mut ShellState,
    msg_counter: &mut u64,
    stdout: &mut Stdout,
) -> io::Result<()> {
    let arg = args.trim();
    if arg.is_empty() {
        match request_recent_conversations(next_control_id(msg_counter), RECENT_CONVERSATION_LIMIT)
        {
            Ok(recent) => {
                render_recent_conversations(
                    stdout,
                    &recent,
                    shell_state.active_conversation.as_deref(),
                )?;
            }
            Err(err) => {
                writeln!(stdout, "{}", round_trip_error_text(err).red())?;
            }
        }
        return Ok(());
    }

    if arg == "new" {
        shell_state.active_conversation = None;
        writeln!(
            stdout,
            "{}",
            "future prompts will start fresh conversations".dark_grey()
        )?;
        return Ok(());
    }

    let recent =
        match request_recent_conversations(next_control_id(msg_counter), RECENT_CONVERSATION_LIMIT)
        {
            Ok(recent) => recent,
            Err(err) => {
                writeln!(stdout, "{}", round_trip_error_text(err).red())?;
                return Ok(());
            }
        };
    match resolve_recent_conversation(&recent, arg) {
        Ok(conversation) => {
            shell_state.active_conversation = Some(conversation.conversation_id.clone());
            writeln!(
                stdout,
                "{}",
                format!(
                    "resuming {} [{}]",
                    short_id(&conversation.conversation_id),
                    conversation.last_task_status
                )
                .green()
            )?;
            if !conversation.last_user_text.is_empty() {
                writeln!(
                    stdout,
                    "{}",
                    format!("last prompt: {}", conversation.last_user_text).dark_grey()
                )?;
            }
        }
        Err(message) => {
            writeln!(stdout, "{}", message.red())?;
        }
    }
    Ok(())
}

fn render_recent_conversations(
    stdout: &mut Stdout,
    conversations: &[ConversationSummary],
    active_conversation: Option<&str>,
) -> io::Result<()> {
    if conversations.is_empty() {
        writeln!(stdout, "{}", "no saved conversations yet".dark_grey())?;
        return Ok(());
    }

    writeln!(stdout, "{}", "recent conversations".bold().cyan())?;
    for conversation in conversations {
        let marker = if active_conversation == Some(conversation.conversation_id.as_str()) {
            "*"
        } else {
            " "
        };
        let preview = if conversation.last_user_text.is_empty() {
            "(no prompt recorded)"
        } else {
            &conversation.last_user_text
        };
        writeln!(
            stdout,
            "{} {} [{}] {}",
            marker,
            short_id(&conversation.conversation_id),
            conversation.last_task_status,
            preview
        )?;
    }
    writeln!(
        stdout,
        "{}",
        "Use /conversations <id-prefix> to resume, or /conversations new for a fresh thread."
            .dark_grey()
    )?;
    Ok(())
}

fn request_recent_conversations(
    id: String,
    limit: u32,
) -> Result<Vec<ConversationSummary>, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::ListConversations {
        id: id.clone(),
        limit,
    });
    write_frame(&mut stream, &req).map_err(|e| RoundTripError::Protocol(e.to_string()))?;

    loop {
        let frame: ServerFrame = match read_frame(&mut stream) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(RoundTripError::Protocol(
                    "bunzod closed the connection before replying".into(),
                ));
            }
            Err(e) => return Err(RoundTripError::Protocol(e.to_string())),
        };

        match frame.msg {
            ServerMessage::ConversationList {
                id: list_id,
                conversations,
            } if list_id == id => return Ok(conversations),
            ServerMessage::ConversationList { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            ServerMessage::TaskList { .. } => {}
            _ => {}
        }
    }
}

fn handle_tasks_command(args: &str, msg_counter: &mut u64, stdout: &mut Stdout) -> io::Result<()> {
    if !args.trim().is_empty() {
        writeln!(stdout, "{}", "usage: /tasks".dark_grey())?;
        return Ok(());
    }

    match request_recent_tasks(next_control_id(msg_counter), RECENT_TASK_LIMIT) {
        Ok(tasks) => render_recent_tasks(stdout, &tasks)?,
        Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
    }
    Ok(())
}

fn render_recent_tasks(stdout: &mut Stdout, tasks: &[TaskSummary]) -> io::Result<()> {
    if tasks.is_empty() {
        writeln!(stdout, "{}", "no saved tasks yet".dark_grey())?;
        return Ok(());
    }

    writeln!(stdout, "{}", "recent tasks".bold().cyan())?;
    for task in tasks {
        let summary = if task.summary.is_empty() {
            "(no summary)"
        } else {
            &task.summary
        };
        let status = if task.task_status == task.run_status {
            task.task_status.as_str().to_string()
        } else {
            format!("{}/{}", task.task_status, task.run_status)
        };
        writeln!(
            stdout,
            "{} [{}] conv:{} {}",
            short_id(&task.task_id),
            status,
            short_id(&task.conversation_id),
            summary
        )?;
        if let Some(reason) = task.state_reason_text.as_deref() {
            if !reason.is_empty() {
                writeln!(stdout, "{}", format!("  reason: {reason}").dark_grey())?;
            }
        }
        if task.snapshot_kind.is_some() {
            writeln!(stdout, "{}", "  resumable snapshot saved".dark_grey())?;
        }
    }
    Ok(())
}

fn request_recent_tasks(id: String, limit: u32) -> Result<Vec<TaskSummary>, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::ListTasks {
        id: id.clone(),
        limit,
    });
    write_frame(&mut stream, &req).map_err(|e| RoundTripError::Protocol(e.to_string()))?;

    loop {
        let frame: ServerFrame = match read_frame(&mut stream) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(RoundTripError::Protocol(
                    "bunzod closed the connection before replying".into(),
                ));
            }
            Err(e) => return Err(RoundTripError::Protocol(e.to_string())),
        };

        match frame.msg {
            ServerMessage::TaskList { id: list_id, tasks } if list_id == id => return Ok(tasks),
            ServerMessage::TaskList { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            ServerMessage::ConversationList { .. } => {}
            _ => {}
        }
    }
}

fn resolve_recent_conversation<'a>(
    conversations: &'a [ConversationSummary],
    prefix: &str,
) -> Result<&'a ConversationSummary, String> {
    let mut matches = conversations
        .iter()
        .filter(|conversation| conversation.conversation_id.starts_with(prefix));
    let first = matches
        .next()
        .ok_or_else(|| format!("no recent conversation matches '{prefix}'"))?;
    if matches.next().is_some() {
        return Err(format!(
            "conversation prefix '{prefix}' is ambiguous in the recent list"
        ));
    }
    Ok(first)
}

fn next_control_id(msg_counter: &mut u64) -> String {
    *msg_counter = msg_counter.wrapping_add(1);
    format!("ctl{}", *msg_counter)
}

fn command_args<'a>(input: &'a str, commands: &[&str]) -> Option<&'a str> {
    for command in commands {
        if input == *command {
            return Some("");
        }
        if let Some(rest) = input.strip_prefix(command) {
            if rest.starts_with(' ') || rest.starts_with('\t') {
                return Some(rest);
            }
        }
    }
    None
}

fn short_id(id: &str) -> &str {
    let end = id
        .char_indices()
        .nth(12)
        .map(|(idx, _)| idx)
        .unwrap_or(id.len());
    &id[..end]
}

fn round_trip_error_text(err: RoundTripError) -> String {
    match err {
        RoundTripError::Unreachable(reason) => format!("bunzod unreachable: {reason}"),
        RoundTripError::Protocol(reason) => format!("protocol error: {reason}"),
        RoundTripError::Remote { code, text } => format!("[{code}] {text}"),
    }
}

fn local_setup_issue() -> Option<String> {
    if !Path::new(BUNZOD_CONFIG_PATH).is_file() {
        return Some("OpenAI backend config is missing.".into());
    }
    match fs::read_to_string(OPENAI_KEY_PATH) {
        Ok(key) if !key.trim().is_empty() => None,
        Ok(_) => Some("OpenAI API key is empty.".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some("OpenAI API key is missing.".into()),
        Err(e) => Some(format!("OpenAI API key is unreadable: {e}")),
    }
}

fn should_offer_setup(code: &str, text: &str) -> bool {
    matches!(code, "unconfigured" | "backend_init_failed")
        || text.contains(BUNZOD_CONFIG_PATH)
        || text.contains(OPENAI_KEY_PATH)
        || text.contains("unsupported OpenAI model")
}

fn run_openai_setup(
    stdin: &mut impl BufRead,
    stdout: &mut Stdout,
    reason: Option<&str>,
) -> io::Result<bool> {
    writeln!(stdout)?;
    writeln!(
        stdout,
        "{}",
        "bunzo setup — OpenAI API key required".bold().cyan()
    )?;
    if let Some(reason) = reason {
        writeln!(stdout, "{}", reason.dark_grey())?;
    }
    writeln!(
        stdout,
        "{}",
        format!(
            "Paste your OpenAI API key. bunzo will save it to {} and configure {}.",
            OPENAI_KEY_PATH, DEFAULT_REMOTE_MODEL
        )
        .dark_grey()
    )?;
    writeln!(stdout, "{}", "Leave it blank to cancel.".dark_grey())?;
    write!(stdout, "{} ", "api key>".cyan().bold())?;
    stdout.flush()?;

    let key = read_secret_line(stdin, stdout)?;
    if key.trim().is_empty() {
        writeln!(stdout, "{}", "setup cancelled".dark_grey())?;
        return Ok(false);
    }

    write_openai_setup(&key)?;
    writeln!(
        stdout,
        "{}",
        format!(
            "saved API key and configured bunzod to use {}",
            DEFAULT_REMOTE_MODEL
        )
        .green()
    )?;
    Ok(true)
}

fn read_secret_line(stdin: &mut impl BufRead, stdout: &mut Stdout) -> io::Result<String> {
    let _echo_guard = StdinEchoGuard::hide().ok();
    let mut line = String::new();
    let bytes = stdin.read_line(&mut line)?;
    writeln!(stdout)?;
    stdout.flush()?;
    if bytes == 0 {
        return Ok(String::new());
    }
    Ok(line.trim().to_string())
}

fn write_openai_setup(key: &str) -> io::Result<()> {
    fs::create_dir_all(BUNZOD_CONFIG_DIR)?;
    fs::set_permissions(BUNZOD_CONFIG_DIR, fs::Permissions::from_mode(0o755))?;
    write_file_with_mode(
        BUNZOD_CONFIG_PATH,
        &format!(
            concat!(
                "# Written by bunzo-shell setup.\n",
                "[backend]\n",
                "kind = \"openai\"\n",
                "model = \"{}\"\n",
                "api_key_path = \"{}\"\n",
            ),
            DEFAULT_REMOTE_MODEL, OPENAI_KEY_PATH
        ),
        0o644,
    )?;
    write_file_with_mode(OPENAI_KEY_PATH, &format!("{key}\n"), 0o600)?;
    Ok(())
}

fn write_file_with_mode(path: &str, contents: &str, mode: u32) -> io::Result<()> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

struct StdinEchoGuard {
    original: libc::termios,
}

impl StdinEchoGuard {
    fn hide() -> io::Result<Self> {
        let fd = libc::STDIN_FILENO;
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        let rc = unsafe { libc::tcgetattr(fd, &mut original) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut hidden = original;
        hidden.c_lflag &= !libc::ECHO;
        let rc = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { original })
    }
}

impl Drop for StdinEchoGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
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
                KeyCode::Enter => {
                    // TUI path is behind BUNZO_SHELL_MODE=tui and deferred to a
                    // later milestone; keep the echo stub here so the code
                    // path still compiles. Real wiring happens in serial mode.
                    let msg = std::mem::take(&mut app.input);
                    let msg = msg.trim().to_string();
                    if !msg.is_empty() {
                        app.history.push((Role::User, msg.clone()));
                        app.history.push((Role::Bunzo, format!("(tui stub) {msg}")));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_offer_matches_config_errors() {
        assert!(should_offer_setup(
            "unconfigured",
            "reading /etc/bunzo/bunzod.toml"
        ));
        assert!(should_offer_setup(
            "backend_init_failed",
            "reading api key from /etc/bunzo/openai.key"
        ));
        assert!(should_offer_setup(
            "backend_error",
            "unsupported OpenAI model 'gpt-4o-mini'"
        ));
        assert!(!should_offer_setup("backend_error", "rate limited"));
    }

    #[test]
    fn setup_writes_expected_config() {
        let cfg = format!(
            concat!(
                "# Written by bunzo-shell setup.\n",
                "[backend]\n",
                "kind = \"openai\"\n",
                "model = \"{}\"\n",
                "api_key_path = \"{}\"\n",
            ),
            DEFAULT_REMOTE_MODEL, OPENAI_KEY_PATH
        );
        assert!(cfg.contains("model = \"gpt-5.4-mini\""));
        assert!(cfg.contains("api_key_path = \"/etc/bunzo/openai.key\""));
    }
}
