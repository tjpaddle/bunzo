use std::env;
use std::fs;
use std::io::{self, BufRead, Stdout, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bunzo_proto::{
    read_frame, write_frame, ClientMessage, ConversationSummary, Envelope, PolicySummary,
    ProvisionClientMessage, ProvisionServerFrame, ProvisionServerMessage, ProvisioningStatus,
    ScheduledJobSummary, ServerFrame, ServerMessage, TaskSummary,
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
const PROVISIOND_SOCKET: &str = "/run/bunzo-provisiond.sock";
const BUNZOD_CONFIG_PATH: &str = "/etc/bunzo/bunzod.toml";
const DEFAULT_REMOTE_MODEL: &str = "gpt-5.4-mini";
const DEFAULT_POLICY_SUBJECT: &str = "shell_request";
const SCHEDULED_JOB_POLICY_SUBJECT: &str = "scheduled_job";
const RECENT_CONVERSATION_LIMIT: u32 = 12;
const RECENT_TASK_LIMIT: u32 = 16;
const RECENT_POLICY_LIMIT: u32 = 24;
const RECENT_JOB_LIMIT: u32 = 24;

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
        if let Some(args) = command_args(input, &["/approvals"]) {
            handle_approvals_command(args, &mut msg_counter, &mut stdout)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }
        if let Some(args) = command_args(input, &["/approve"]) {
            handle_approve_command(args, &mut shell_state, &mut msg_counter, &mut stdout)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }
        if let Some(args) = command_args(input, &["/policy", "/policies"]) {
            handle_policy_command(args, &shell_state, &mut msg_counter, &mut stdout)?;
            writeln!(stdout)?;
            stdout.flush()?;
            continue;
        }
        if let Some(args) = command_args(input, &["/jobs"]) {
            handle_jobs_command(args, &mut msg_counter, &mut stdout)?;
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
            Ok(outcome) => apply_round_trip_outcome(&mut shell_state, outcome, &mut stdout)?,
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
                            apply_round_trip_outcome(&mut shell_state, outcome, &mut stdout)?
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

enum ProvisioningRoundTripError {
    Unreachable(String),
    Protocol(String),
    Remote { code: String, text: String },
}

fn apply_round_trip_outcome<W: Write>(
    shell_state: &mut ShellState,
    outcome: RoundTripOutcome,
    stdout: &mut W,
) -> io::Result<()> {
    let was_tracking = shell_state.active_conversation.is_some();
    let conversation_id = outcome.conversation_id;
    let created_conversation = outcome.created_conversation;

    shell_state.active_conversation = Some(conversation_id.clone());

    if !was_tracking && created_conversation {
        writeln!(
            stdout,
            "{}",
            format!(
                "[saved as {} — use /conversations new for a fresh thread]",
                short_id(&conversation_id)
            )
            .dark_grey()
        )?;
    }

    Ok(())
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
            ServerMessage::PolicyDecision {
                id: policy_id,
                action,
                resource,
                decision,
                grant_scope,
                detail,
                ..
            } if policy_id == id => {
                render_policy_decision(
                    stdout,
                    &action,
                    &resource,
                    &decision,
                    &grant_scope,
                    &detail,
                )
                .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::PolicyDecision { .. } => {}
            ServerMessage::ConversationList { .. } => {}
            ServerMessage::TaskList { .. } => {}
            ServerMessage::PolicyList { .. } => {}
            ServerMessage::PolicyMutationResult { .. } => {}
            ServerMessage::PolicyDeleteResult { .. } => {}
            ServerMessage::ScheduledJobList { .. } => {}
            ServerMessage::ScheduledJobMutationResult { .. } => {}
            ServerMessage::ScheduledJobDeleteResult { .. } => {}
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

fn render_policy_decision(
    stdout: &mut Stdout,
    action: &str,
    resource: &str,
    decision: &str,
    grant_scope: &str,
    detail: &str,
) -> io::Result<()> {
    writeln!(stdout)?;
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    let line = match decision {
        "deny" => format!("policy denied {action} {resource} [{grant_scope}]{suffix}")
            .red()
            .italic()
            .to_string(),
        "require_approval" => {
            format!("approval needed for {action} {resource} [{grant_scope}]{suffix}")
                .yellow()
                .italic()
                .to_string()
        }
        _ => format!("policy {decision} {action} {resource} [{grant_scope}]")
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

fn handle_approvals_command(
    args: &str,
    msg_counter: &mut u64,
    stdout: &mut Stdout,
) -> io::Result<()> {
    if !args.trim().is_empty() {
        writeln!(stdout, "{}", "usage: /approvals".dark_grey())?;
        return Ok(());
    }

    match request_recent_tasks(next_control_id(msg_counter), RECENT_TASK_LIMIT) {
        Ok(tasks) => render_waiting_approvals(stdout, &tasks)?,
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
    let mut approval_index = 0usize;
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
            "{} [{} {}] conv:{} {}",
            short_id(&task.task_id),
            task_kind_label(&task.task_kind),
            status,
            short_id(&task.conversation_id),
            summary
        )?;
        if !task.task_run_id.is_empty() {
            writeln!(
                stdout,
                "{}",
                format!("  run: {}", short_id(&task.task_run_id)).dark_grey()
            )?;
        }
        if let Some(reason) = task.state_reason_text.as_deref() {
            if !reason.is_empty() {
                writeln!(stdout, "{}", format!("  reason: {reason}").dark_grey())?;
            }
        }
        if task.snapshot_kind.is_some() {
            writeln!(stdout, "{}", "  resumable snapshot saved".dark_grey())?;
        }
        if is_waiting_approval(task) {
            approval_index += 1;
            let label = if approval_index == 1 {
                "#1 (latest)".to_string()
            } else {
                format!("#{approval_index}")
            };
            writeln!(stdout, "{}", format!("  approval: {label}").dark_grey())?;
            writeln!(
                stdout,
                "{}",
                format!(
                    "  approve: /approve {} <once|task|session|persistent>",
                    approval_index
                )
                .dark_grey()
            )?;
        }
    }
    Ok(())
}

fn render_waiting_approvals(stdout: &mut Stdout, tasks: &[TaskSummary]) -> io::Result<()> {
    let approvals = waiting_approval_tasks(tasks);
    if approvals.is_empty() {
        writeln!(stdout, "{}", "no waiting approvals".dark_grey())?;
        return Ok(());
    }

    writeln!(stdout, "{}", "waiting approvals".bold().cyan())?;
    for (index, task) in approvals.iter().enumerate() {
        let approval_number = index + 1;
        let summary = if task.summary.is_empty() {
            "(no summary)"
        } else {
            &task.summary
        };
        let label = if approval_number == 1 {
            "1 (latest)".to_string()
        } else {
            approval_number.to_string()
        };
        writeln!(
            stdout,
            "{}. {} run:{} conv:{} {}",
            label,
            task_kind_label(&task.task_kind),
            short_id(&task.task_run_id),
            short_id(&task.conversation_id),
            summary
        )?;
        if let Some(reason) = task.state_reason_text.as_deref() {
            if !reason.is_empty() {
                writeln!(stdout, "{}", format!("  reason: {reason}").dark_grey())?;
            }
        }
        writeln!(
            stdout,
            "{}",
            format!(
                "  approve: /approve {} <once|task|session|persistent>",
                approval_number
            )
            .dark_grey()
        )?;
    }
    writeln!(stdout, "{}", approve_usage().dark_grey())?;
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

fn handle_policy_command(
    args: &str,
    shell_state: &ShellState,
    msg_counter: &mut u64,
    stdout: &mut Stdout,
) -> io::Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        match request_runtime_policies(next_control_id(msg_counter), RECENT_POLICY_LIMIT) {
            Ok(policies) => render_runtime_policies(stdout, &policies)?,
            Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
        }
        return Ok(());
    }

    let mut parts = trimmed.split_whitespace();
    let subcommand = parts.next().unwrap_or_default();
    match subcommand {
        "delete" | "rm" => {
            let Some(policy_id) = parts.next() else {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            };
            if parts.next().is_some() {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            }
            match request_delete_policy(next_control_id(msg_counter), policy_id.to_string()) {
                Ok(deleted_policy_id) => writeln!(
                    stdout,
                    "{}",
                    format!("deleted policy {}", short_id(&deleted_policy_id)).green()
                )?,
                Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
            }
        }
        "allow" | "deny" | "require-approval" => {
            let Some(first_arg) = parts.next() else {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            };
            let Some((subject, resource)) =
                parse_policy_subject_and_resource(first_arg, &mut parts)
            else {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            };
            let Some(scope) = parts.next() else {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            };
            let mut target = if scope == "persistent" {
                None
            } else {
                parts.next().map(str::to_string)
            };
            if subject == DEFAULT_POLICY_SUBJECT && scope == "session" && target.is_none() {
                target = shell_state.active_conversation.clone();
            }
            if matches!(scope, "session" | "task" | "once") && target.is_none() {
                writeln!(stdout, "{}", policy_usage().dark_grey())?;
                return Ok(());
            }
            let note_text = {
                let rest = parts.collect::<Vec<_>>().join(" ");
                if rest.is_empty() {
                    Some(format!(
                        "set by bunzo-shell to {} for {}",
                        subcommand, resource
                    ))
                } else {
                    Some(rest)
                }
            };
            let decision = if subcommand == "require-approval" {
                "require_approval"
            } else {
                subcommand
            };
            match request_upsert_policy(
                next_control_id(msg_counter),
                subject,
                "invoke_skill".into(),
                resource.to_string(),
                decision.to_string(),
                scope.to_string(),
                target,
                note_text,
            ) {
                Ok((policy, created)) => render_policy_mutation(stdout, &policy, created)?,
                Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
            }
        }
        _ => {
            writeln!(stdout, "{}", policy_usage().dark_grey())?;
        }
    }

    Ok(())
}

fn handle_jobs_command(args: &str, msg_counter: &mut u64, stdout: &mut Stdout) -> io::Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        match request_scheduled_jobs(next_control_id(msg_counter), RECENT_JOB_LIMIT) {
            Ok(jobs) => render_scheduled_jobs(stdout, &jobs)?,
            Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
        }
        return Ok(());
    }

    let mut parts = trimmed.split_whitespace();
    let subcommand = parts.next().unwrap_or_default();
    match subcommand {
        "every" => {
            let Some(interval_text) = parts.next() else {
                writeln!(stdout, "{}", jobs_usage().dark_grey())?;
                return Ok(());
            };
            let interval_seconds = match parse_job_interval_seconds(interval_text) {
                Ok(seconds) => seconds,
                Err(reason) => {
                    writeln!(stdout, "{}", reason.red())?;
                    return Ok(());
                }
            };
            let prompt = parts.collect::<Vec<_>>().join(" ");
            if prompt.trim().is_empty() {
                writeln!(stdout, "{}", jobs_usage().dark_grey())?;
                return Ok(());
            }
            let name = truncate_text(prompt.trim(), 48);
            match request_create_scheduled_job(
                next_control_id(msg_counter),
                name,
                prompt,
                interval_seconds,
            ) {
                Ok(job) => render_scheduled_job_mutation(stdout, &job)?,
                Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
            }
        }
        "delete" | "rm" => {
            let Some(job_id) = parts.next() else {
                writeln!(stdout, "{}", jobs_usage().dark_grey())?;
                return Ok(());
            };
            if parts.next().is_some() {
                writeln!(stdout, "{}", jobs_usage().dark_grey())?;
                return Ok(());
            }
            match request_delete_scheduled_job(next_control_id(msg_counter), job_id.to_string()) {
                Ok(job_id) => writeln!(
                    stdout,
                    "{}",
                    format!("deleted job {}", short_id(&job_id)).green()
                )?,
                Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
            }
        }
        _ => {
            writeln!(stdout, "{}", jobs_usage().dark_grey())?;
        }
    }

    Ok(())
}

fn handle_approve_command(
    args: &str,
    shell_state: &mut ShellState,
    msg_counter: &mut u64,
    stdout: &mut Stdout,
) -> io::Result<()> {
    let mut parts = args.trim().split_whitespace();
    let Some(requested_target) = parts.next() else {
        writeln!(stdout, "{}", approve_usage().dark_grey())?;
        return Ok(());
    };
    let Some(grant_scope) = parts.next() else {
        writeln!(stdout, "{}", approve_usage().dark_grey())?;
        return Ok(());
    };
    let note_text = {
        let rest = parts.collect::<Vec<_>>().join(" ");
        if rest.is_empty() {
            None
        } else {
            Some(rest)
        }
    };
    let task_run_id = if requested_target.eq_ignore_ascii_case("latest")
        || requested_target.parse::<usize>().is_ok()
    {
        match request_recent_tasks(next_control_id(msg_counter), RECENT_TASK_LIMIT) {
            Ok(tasks) => match resolve_waiting_approval_alias(&tasks, requested_target) {
                Ok(task) => task.task_run_id.clone(),
                Err(reason) => {
                    writeln!(stdout, "{}", reason.red())?;
                    return Ok(());
                }
            },
            Err(err) => {
                writeln!(stdout, "{}", round_trip_error_text(err).red())?;
                return Ok(());
            }
        }
    } else {
        requested_target.to_string()
    };

    match request_approval_resolution(
        next_control_id(msg_counter),
        task_run_id,
        grant_scope.to_string(),
        note_text,
        stdout,
    ) {
        Ok(outcome) => apply_round_trip_outcome(shell_state, outcome, stdout)?,
        Err(err) => writeln!(stdout, "{}", round_trip_error_text(err).red())?,
    }

    Ok(())
}

fn render_runtime_policies(stdout: &mut Stdout, policies: &[PolicySummary]) -> io::Result<()> {
    if policies.is_empty() {
        writeln!(stdout, "{}", "no runtime policies yet".dark_grey())?;
        return Ok(());
    }

    writeln!(stdout, "{}", "runtime policies".bold().cyan())?;
    for policy in policies {
        writeln!(
            stdout,
            "{} [{}/{}] {} {} {}",
            short_id(&policy.policy_id),
            policy.decision,
            policy.grant_scope,
            policy.subject,
            policy.action,
            policy.resource
        )?;
        if let Some(note_text) = policy.note_text.as_deref() {
            if !note_text.is_empty() {
                writeln!(stdout, "{}", format!("  note: {note_text}").dark_grey())?;
            }
        }
        let mut targets = Vec::new();
        if let Some(conversation_id) = policy.conversation_id.as_deref() {
            targets.push(format!("conv:{}", short_id(conversation_id)));
        }
        if let Some(task_id) = policy.task_id.as_deref() {
            targets.push(format!("task:{}", short_id(task_id)));
        }
        if let Some(task_run_id) = policy.task_run_id.as_deref() {
            targets.push(format!("run:{}", short_id(task_run_id)));
        }
        if !targets.is_empty() {
            writeln!(stdout, "{}", format!("  {}", targets.join(" ")).dark_grey())?;
        }
    }
    writeln!(stdout, "{}", policy_usage().dark_grey())?;
    Ok(())
}

fn render_policy_mutation(
    stdout: &mut Stdout,
    policy: &PolicySummary,
    created: bool,
) -> io::Result<()> {
    let verb = if created { "created" } else { "updated" };
    writeln!(
        stdout,
        "{}",
        format!(
            "{} policy {} [{}/{}] {} {}",
            verb,
            short_id(&policy.policy_id),
            policy.decision,
            policy.grant_scope,
            policy.subject,
            policy.resource
        )
        .green()
    )
}

fn render_scheduled_jobs(stdout: &mut Stdout, jobs: &[ScheduledJobSummary]) -> io::Result<()> {
    if jobs.is_empty() {
        writeln!(stdout, "{}", "no scheduled jobs yet".dark_grey())?;
        writeln!(stdout, "{}", jobs_usage().dark_grey())?;
        return Ok(());
    }

    writeln!(stdout, "{}", "scheduled jobs".bold().cyan())?;
    for job in jobs {
        let enabled = if job.enabled { "active" } else { "deleted" };
        let last_status = job.last_run_status.as_deref().unwrap_or("never-run");
        writeln!(
            stdout,
            "{} [{} every {} {}] {}",
            short_id(&job.job_id),
            enabled,
            format_job_interval(job.interval_seconds),
            format_next_due(job.next_run_at_ms),
            job.name
        )?;
        writeln!(stdout, "{}", format!("  last: {last_status}").dark_grey())?;
        writeln!(
            stdout,
            "{}",
            format!("  prompt: {}", job.prompt_preview).dark_grey()
        )?;
        if let Some(task_run_id) = job.last_task_run_id.as_deref() {
            writeln!(
                stdout,
                "{}",
                format!("  latest run: {}", short_id(task_run_id)).dark_grey()
            )?;
        }
    }
    writeln!(stdout, "{}", jobs_usage().dark_grey())?;
    Ok(())
}

fn render_scheduled_job_mutation(stdout: &mut Stdout, job: &ScheduledJobSummary) -> io::Result<()> {
    writeln!(
        stdout,
        "{}",
        format!(
            "created job {} [every {} {}] {}",
            short_id(&job.job_id),
            format_job_interval(job.interval_seconds),
            format_next_due(job.next_run_at_ms),
            job.name
        )
        .green()
    )
}

fn render_approval_resolution(
    stdout: &mut Stdout,
    policy: &PolicySummary,
    created: bool,
) -> io::Result<()> {
    let verb = if created { "approved" } else { "reapproved" };
    writeln!(
        stdout,
        "{}",
        format!(
            "{} waiting request via policy {} [{}/{}] {}",
            verb,
            short_id(&policy.policy_id),
            policy.decision,
            policy.grant_scope,
            policy.resource
        )
        .green()
    )
}

fn policy_usage() -> &'static str {
    "usage: /policy list | /policy <allow|deny|require-approval> [shell_request|scheduled_job] <resource> <persistent|session|task|once> [target-id-prefix] [note...] | /policy delete <policy-id-prefix>"
}

fn jobs_usage() -> &'static str {
    "usage: /jobs list | /jobs every <seconds> <prompt...> | /jobs delete <job-id-prefix>"
}

fn approve_usage() -> &'static str {
    "usage: /approve <latest|approval-number|task-run-id-prefix> <once|task|session|persistent> [note...]"
}

fn task_kind_label(task_kind: &str) -> &str {
    match task_kind {
        "shell_request" => "shell",
        "scheduled_job" => "job",
        _ => task_kind,
    }
}

fn parse_policy_subject_and_resource<'a>(
    first_arg: &'a str,
    parts: &mut std::str::SplitWhitespace<'a>,
) -> Option<(String, &'a str)> {
    match first_arg {
        DEFAULT_POLICY_SUBJECT | SCHEDULED_JOB_POLICY_SUBJECT => {
            let resource = parts.next()?;
            Some((first_arg.to_string(), resource))
        }
        _ => Some((DEFAULT_POLICY_SUBJECT.into(), first_arg)),
    }
}

fn request_runtime_policies(id: String, limit: u32) -> Result<Vec<PolicySummary>, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::ListPolicies {
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
            ServerMessage::PolicyList {
                id: list_id,
                policies,
            } if list_id == id => return Ok(policies),
            ServerMessage::PolicyList { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_scheduled_jobs(
    id: String,
    limit: u32,
) -> Result<Vec<ScheduledJobSummary>, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::ListScheduledJobs {
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
            ServerMessage::ScheduledJobList { id: list_id, jobs } if list_id == id => {
                return Ok(jobs);
            }
            ServerMessage::ScheduledJobList { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_provisioning_status(
    id: String,
) -> Result<ProvisioningStatus, ProvisioningRoundTripError> {
    let mut stream = UnixStream::connect(PROVISIOND_SOCKET)
        .map_err(|e| ProvisioningRoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ProvisionClientMessage::GetProvisioningStatus { id: id.clone() });
    write_frame(&mut stream, &req)
        .map_err(|e| ProvisioningRoundTripError::Protocol(e.to_string()))?;

    loop {
        let frame: ProvisionServerFrame = match read_frame(&mut stream) {
            Ok(frame) => frame,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(ProvisioningRoundTripError::Protocol(
                    "bunzo-provisiond closed the connection before replying".into(),
                ));
            }
            Err(e) => return Err(ProvisioningRoundTripError::Protocol(e.to_string())),
        };

        match frame.msg {
            ProvisionServerMessage::ProvisioningStatus {
                id: result_id,
                status,
            } if result_id == id => return Ok(status),
            ProvisionServerMessage::ProvisioningStatus { .. } => {}
            ProvisionServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(ProvisioningRoundTripError::Remote { code, text });
            }
            ProvisionServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn apply_local_openai_setup(
    id: String,
    device_name: Option<String>,
    api_key: String,
) -> Result<ProvisioningStatus, ProvisioningRoundTripError> {
    let mut stream = UnixStream::connect(PROVISIOND_SOCKET)
        .map_err(|e| ProvisioningRoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ProvisionClientMessage::ApplyLocalSetup {
        id: id.clone(),
        device_name,
        api_key,
    });
    write_frame(&mut stream, &req)
        .map_err(|e| ProvisioningRoundTripError::Protocol(e.to_string()))?;

    loop {
        let frame: ProvisionServerFrame = match read_frame(&mut stream) {
            Ok(frame) => frame,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(ProvisioningRoundTripError::Protocol(
                    "bunzo-provisiond closed the connection before replying".into(),
                ));
            }
            Err(e) => return Err(ProvisioningRoundTripError::Protocol(e.to_string())),
        };

        match frame.msg {
            ProvisionServerMessage::ProvisioningResult {
                id: result_id,
                status,
            } if result_id == id => return Ok(status),
            ProvisionServerMessage::ProvisioningResult { .. } => {}
            ProvisionServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(ProvisioningRoundTripError::Remote { code, text });
            }
            ProvisionServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_upsert_policy(
    id: String,
    subject: String,
    action: String,
    resource: String,
    decision: String,
    grant_scope: String,
    target: Option<String>,
    note_text: Option<String>,
) -> Result<(PolicySummary, bool), RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::UpsertPolicy {
        id: id.clone(),
        subject,
        action,
        resource,
        decision,
        grant_scope,
        target,
        note_text,
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
            ServerMessage::PolicyMutationResult {
                id: result_id,
                policy,
                created,
            } if result_id == id => return Ok((policy, created)),
            ServerMessage::PolicyMutationResult { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_create_scheduled_job(
    id: String,
    name: String,
    prompt: String,
    interval_seconds: u64,
) -> Result<ScheduledJobSummary, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::CreateScheduledJob {
        id: id.clone(),
        name,
        prompt,
        interval_seconds,
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
            ServerMessage::ScheduledJobMutationResult { id: result_id, job } if result_id == id => {
                return Ok(job);
            }
            ServerMessage::ScheduledJobMutationResult { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_delete_policy(id: String, policy_id: String) -> Result<String, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::DeletePolicy {
        id: id.clone(),
        policy_id,
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
            ServerMessage::PolicyDeleteResult {
                id: result_id,
                policy_id,
            } if result_id == id => return Ok(policy_id),
            ServerMessage::PolicyDeleteResult { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_delete_scheduled_job(id: String, job_id: String) -> Result<String, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::DeleteScheduledJob {
        id: id.clone(),
        job_id,
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
            ServerMessage::ScheduledJobDeleteResult {
                id: result_id,
                job_id,
            } if result_id == id => {
                return Ok(job_id);
            }
            ServerMessage::ScheduledJobDeleteResult { .. } => {}
            ServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(RoundTripError::Remote { code, text });
            }
            ServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

fn request_approval_resolution(
    id: String,
    task_run_id: String,
    grant_scope: String,
    note_text: Option<String>,
    stdout: &mut Stdout,
) -> Result<RoundTripOutcome, RoundTripError> {
    let mut stream = UnixStream::connect(BUNZOD_SOCKET)
        .map_err(|e| RoundTripError::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let req = Envelope::new(ClientMessage::ResolveApproval {
        id: id.clone(),
        task_run_id,
        grant_scope,
        note_text,
    });
    write_frame(&mut stream, &req).map_err(|e| RoundTripError::Protocol(e.to_string()))?;

    let mut outcome: Option<RoundTripOutcome> = None;
    let mut assistant_tag_open = false;

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
            ServerMessage::PolicyMutationResult {
                id: result_id,
                policy,
                created,
            } if result_id == id => {
                render_approval_resolution(stdout, &policy, created)
                    .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::PolicyMutationResult { .. } => {}
            ServerMessage::AssistantChunk { id: chunk_id, text } if chunk_id == id => {
                if !assistant_tag_open {
                    write!(stdout, "{} ", "bunzo".bold().magenta())
                        .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
                    assistant_tag_open = true;
                }
                write!(stdout, "{text}").map_err(|e| RoundTripError::Protocol(e.to_string()))?;
                stdout
                    .flush()
                    .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
            }
            ServerMessage::AssistantChunk { .. } => {}
            ServerMessage::AssistantEnd { id: end_id, .. } if end_id == id => {
                return outcome.ok_or_else(|| {
                    RoundTripError::Protocol(
                        "bunzod ended the approval flow without request context".into(),
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
                assistant_tag_open = true;
            }
            ServerMessage::ToolActivity { .. } => {}
            ServerMessage::PolicyDecision {
                id: policy_id,
                action,
                resource,
                decision,
                grant_scope,
                detail,
                ..
            } if policy_id == id => {
                render_policy_decision(
                    stdout,
                    &action,
                    &resource,
                    &decision,
                    &grant_scope,
                    &detail,
                )
                .map_err(|e| RoundTripError::Protocol(e.to_string()))?;
                assistant_tag_open = true;
            }
            ServerMessage::PolicyDecision { .. } => {}
            ServerMessage::ConversationList { .. } => {}
            ServerMessage::TaskList { .. } => {}
            ServerMessage::PolicyList { .. } => {}
            ServerMessage::PolicyDeleteResult { .. } => {}
            ServerMessage::ScheduledJobList { .. } => {}
            ServerMessage::ScheduledJobMutationResult { .. } => {}
            ServerMessage::ScheduledJobDeleteResult { .. } => {}
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

fn is_waiting_approval(task: &TaskSummary) -> bool {
    task.state_reason_code.as_deref() == Some("policy_approval_required")
        && !task.task_run_id.is_empty()
}

fn waiting_approval_tasks<'a>(tasks: &'a [TaskSummary]) -> Vec<&'a TaskSummary> {
    tasks
        .iter()
        .filter(|task| is_waiting_approval(task))
        .collect()
}

fn resolve_waiting_approval_alias<'a>(
    tasks: &'a [TaskSummary],
    requested: &str,
) -> Result<&'a TaskSummary, String> {
    let approvals = waiting_approval_tasks(tasks);
    if approvals.is_empty() {
        return Err("no waiting approvals in the recent task list".into());
    }

    if requested.eq_ignore_ascii_case("latest") {
        return Ok(approvals[0]);
    }

    let index = requested.parse::<usize>().map_err(|_| {
        format!("approval alias '{requested}' must be 'latest' or a positive number")
    })?;
    if index == 0 {
        return Err("approval number must start at 1".into());
    }
    approvals
        .get(index - 1)
        .copied()
        .ok_or_else(|| format!("no waiting approval #{index} in the recent task list"))
}

fn next_control_id(msg_counter: &mut u64) -> String {
    *msg_counter = msg_counter.wrapping_add(1);
    format!("ctl{}", *msg_counter)
}

fn parse_job_interval_seconds(input: &str) -> Result<u64, String> {
    let seconds = input
        .parse::<u64>()
        .map_err(|_| format!("job interval '{input}' must be an integer number of seconds"))?;
    if seconds < 5 {
        return Err("job interval must be at least 5 seconds".into());
    }
    Ok(seconds)
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

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_job_interval(seconds: u64) -> String {
    if seconds % 3600 == 0 {
        format!("{}h", seconds / 3600)
    } else if seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn format_next_due(next_run_at_ms: u64) -> String {
    let now = now_ms();
    if next_run_at_ms <= now {
        return "due now".into();
    }
    let remaining = (next_run_at_ms - now) / 1000;
    format!("next in {}", format_job_interval(remaining.max(1)))
}

fn round_trip_error_text(err: RoundTripError) -> String {
    match err {
        RoundTripError::Unreachable(reason) => format!("bunzod unreachable: {reason}"),
        RoundTripError::Protocol(reason) => format!("protocol error: {reason}"),
        RoundTripError::Remote { code, text } => format!("[{code}] {text}"),
    }
}

fn provisioning_error_text(err: ProvisioningRoundTripError) -> String {
    match err {
        ProvisioningRoundTripError::Unreachable(reason) => {
            format!("bunzo-provisiond unreachable: {reason}")
        }
        ProvisioningRoundTripError::Protocol(reason) => format!("protocol error: {reason}"),
        ProvisioningRoundTripError::Remote { code, text } => format!("[{code}] {text}"),
    }
}

fn local_setup_issue() -> Option<String> {
    match request_provisioning_status("setup-status".into()) {
        Ok(status) if status.ready => None,
        Ok(status) => Some(provisioning_issue_text(&status)),
        Err(err) => Some(provisioning_error_text(err)),
    }
}

fn should_offer_setup(code: &str, text: &str) -> bool {
    matches!(code, "unconfigured" | "backend_init_failed")
        || text.contains(BUNZOD_CONFIG_PATH)
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
            "Paste your OpenAI API key. bunzo will persist it under /var/lib/bunzo/secrets/ and render {} for {}.",
            BUNZOD_CONFIG_PATH, DEFAULT_REMOTE_MODEL
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

    match apply_local_openai_setup("setup-apply".into(), None, key) {
        Ok(status) => {
            let device_name = status.device_name.as_deref().unwrap_or("this device");
            let rendered_path = status
                .rendered_config_path
                .as_deref()
                .unwrap_or(BUNZOD_CONFIG_PATH);
            writeln!(
                stdout,
                "{}",
                format!(
                    "validated OpenAI access for {device_name} and rendered {rendered_path} for {}",
                    status.model.as_deref().unwrap_or(DEFAULT_REMOTE_MODEL)
                )
                .green()
            )?;
            Ok(true)
        }
        Err(err) => {
            writeln!(
                stdout,
                "{}",
                format!("setup failed: {}", provisioning_error_text(err)).red()
            )?;
            if let Ok(status) = request_provisioning_status("setup-status-after-failure".into()) {
                writeln!(stdout, "{}", provisioning_issue_text(&status).dark_grey())?;
            }
            Ok(false)
        }
    }
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

fn provisioning_issue_text(status: &ProvisioningStatus) -> String {
    let detail = status
        .detail
        .as_deref()
        .unwrap_or("setup has not completed yet");
    let device = status.device_name.as_deref().unwrap_or("this device");
    let provider = status.provider_kind.as_deref().unwrap_or("backend");
    match status.phase.as_str() {
        "failed_recoverable" => format!(
            "{device} is not ready: {provider} validation failed and can be retried: {detail}"
        ),
        "validating" => format!("{device} is still validating {provider}: {detail}"),
        phase => format!("{device} provisioning phase '{phase}' is not ready: {detail}"),
    }
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

    fn task_summary(
        task_id: &str,
        task_run_id: &str,
        state_reason_code: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            task_id: task_id.into(),
            conversation_id: format!("conv-{task_id}"),
            task_run_id: task_run_id.into(),
            task_kind: "shell_request".into(),
            updated_at_ms: 0,
            task_status: "waiting".into(),
            run_status: "waiting".into(),
            summary: format!("summary-{task_id}"),
            state_reason_code: state_reason_code.map(str::to_string),
            state_reason_text: None,
            snapshot_kind: Some("shell_request_waiting_v1".into()),
        }
    }

    #[test]
    fn setup_offer_matches_config_errors() {
        assert!(should_offer_setup(
            "unconfigured",
            "reading /etc/bunzo/bunzod.toml"
        ));
        assert!(should_offer_setup("backend_init_failed", "api key missing"));
        assert!(should_offer_setup(
            "backend_error",
            "unsupported OpenAI model 'gpt-4o-mini'"
        ));
        assert!(!should_offer_setup("backend_error", "rate limited"));
    }

    #[test]
    fn provisioning_issue_text_surfaces_phase_and_detail() {
        let status = ProvisioningStatus {
            phase: "failed_recoverable".into(),
            ready: false,
            device_name: Some("bunzo-qemu".into()),
            connectivity_kind: Some("existing_network".into()),
            provider_kind: Some("openai".into()),
            model: Some(DEFAULT_REMOTE_MODEL.into()),
            rendered_config_path: Some(BUNZOD_CONFIG_PATH.into()),
            secret_path: Some("/var/lib/bunzo/secrets/openai.key".into()),
            detail: Some("reading api key from /var/lib/bunzo/secrets/openai.key".into()),
            updated_at_ms: 0,
        };

        let text = provisioning_issue_text(&status);
        assert!(text.contains("bunzo-qemu"));
        assert!(text.contains("validation failed"));
        assert!(text.contains("/var/lib/bunzo/secrets/openai.key"));
    }

    #[test]
    fn first_successful_prompt_becomes_active_conversation() {
        let mut shell_state = ShellState::default();
        let mut buf = Vec::new();

        let outcome = RoundTripOutcome {
            conversation_id: "c1".into(),
            created_conversation: true,
        };
        apply_round_trip_outcome(&mut shell_state, outcome, &mut buf).unwrap();

        assert_eq!(shell_state.active_conversation.as_deref(), Some("c1"));
        let rendered = String::from_utf8(buf).unwrap();
        assert!(rendered.contains("/conversations new"));
    }

    #[test]
    fn waiting_approval_alias_resolves_latest() {
        let tasks = vec![
            task_summary("t1", "run-1", Some("policy_approval_required")),
            task_summary("t2", "run-2", Some("policy_approval_required")),
        ];

        let resolved = resolve_waiting_approval_alias(&tasks, "latest").unwrap();
        assert_eq!(resolved.task_run_id, "run-1");
    }

    #[test]
    fn waiting_approval_alias_resolves_numeric_index() {
        let tasks = vec![
            task_summary("t1", "run-1", Some("policy_approval_required")),
            task_summary("t2", "run-2", Some("other_wait")),
            task_summary("t3", "run-3", Some("policy_approval_required")),
        ];

        let resolved = resolve_waiting_approval_alias(&tasks, "2").unwrap();
        assert_eq!(resolved.task_run_id, "run-3");
    }

    #[test]
    fn waiting_approval_alias_rejects_missing_index() {
        let tasks = vec![task_summary(
            "t1",
            "run-1",
            Some("policy_approval_required"),
        )];

        let err = resolve_waiting_approval_alias(&tasks, "2").unwrap_err();
        assert!(err.contains("no waiting approval #2"));
    }

    #[test]
    fn parse_policy_subject_defaults_to_shell_request() {
        let mut parts = "read-local-file persistent".split_whitespace();
        let parsed = parse_policy_subject_and_resource(parts.next().unwrap(), &mut parts).unwrap();
        assert_eq!(parsed.0, "shell_request");
        assert_eq!(parsed.1, "read-local-file");
    }

    #[test]
    fn parse_policy_subject_accepts_scheduled_job() {
        let mut parts = "scheduled_job read-local-file persistent".split_whitespace();
        let parsed = parse_policy_subject_and_resource(parts.next().unwrap(), &mut parts).unwrap();
        assert_eq!(parsed.0, "scheduled_job");
        assert_eq!(parsed.1, "read-local-file");
    }
}
