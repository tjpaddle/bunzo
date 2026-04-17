//! bunzod — bunzo's agent daemon.
//!
//! Listens on /run/bunzod.sock (or a systemd-activated socket), speaks the
//! bunzo wire protocol v1 from `bunzo-proto`, and streams replies back to
//! the chat shell via the configured LLM backend. The model can call skills
//! — WASM modules loaded from `/usr/lib/bunzo/skills` at startup — and each
//! completed exchange (including any skill invocations) is appended to the
//! action ledger.

mod backend;
mod config;
mod ledger;
mod skills;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bunzo_proto::async_io::{read_frame_async, write_frame_async};
use bunzo_proto::{
    ClientFrame, ClientMessage, Envelope, ServerMessage, PROTOCOL_VERSION,
};
use listenfd::ListenFd;
use sd_notify::NotifyState;
use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::backend::{BackendEvent, Message, Role};
use crate::ledger::{Entry, Ledger, ToolRecord};
use crate::skills::Registry;

const SOCKET_PATH: &str = "/run/bunzod.sock";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let listener = acquire_listener()?;
    let ledger = Arc::new(Ledger::new(Ledger::default_path()));
    let registry = Registry::load_from(&skills::default_dir());
    eprintln!(
        "bunzod: loaded {} skills",
        registry.tool_descriptors().len()
    );
    eprintln!("bunzod: accepting connections");

    let _ = sd_notify::notify(false, &[NotifyState::Ready]);

    loop {
        let (stream, _addr) = listener.accept().await?;
        let ledger = Arc::clone(&ledger);
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, ledger, registry).await {
                eprintln!("bunzod: connection ended: {e:#}");
            }
        });
    }
}

fn acquire_listener() -> Result<UnixListener> {
    let mut listenfd = ListenFd::from_env();
    if let Some(std_listener) = listenfd.take_unix_listener(0)? {
        std_listener.set_nonblocking(true)?;
        eprintln!("bunzod: using socket-activated listener from systemd");
        return UnixListener::from_std(std_listener).context("wrapping inherited listener");
    }

    let path = Path::new(SOCKET_PATH);
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("removing stale {SOCKET_PATH}"))?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("binding {SOCKET_PATH}"))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660));
    eprintln!("bunzod: bound {SOCKET_PATH} directly");
    Ok(listener)
}

async fn handle_connection(
    mut stream: UnixStream,
    ledger: Arc<Ledger>,
    registry: Registry,
) -> Result<()> {
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    loop {
        let frame: ClientFrame = match read_frame_async(&mut reader).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        if frame.v != PROTOCOL_VERSION {
            let err = Envelope::new(ServerMessage::Error {
                id: String::new(),
                code: "unsupported_version".into(),
                text: format!(
                    "client speaks v{}, bunzod speaks v{PROTOCOL_VERSION}",
                    frame.v
                ),
            });
            write_frame_async(&mut write_half, &err).await?;
            continue;
        }

        match frame.msg {
            ClientMessage::UserMessage { id, text } => {
                handle_user_message(
                    &mut write_half,
                    &id,
                    &text,
                    &ledger,
                    registry.clone(),
                )
                .await?;
            }
            ClientMessage::Cancel { id: _ } => {
                // In-flight request cancellation is not wired yet; the real
                // hook is an abort-handle on the backend task, added when a
                // user-visible cancel UX exists in the shell.
            }
        }
    }
}

async fn handle_user_message<W>(
    w: &mut W,
    id: &str,
    user_text: &str,
    ledger: &Ledger,
    registry: Registry,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let started = Instant::now();

    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            return finish_with_error(w, id, "unconfigured", &format!("{e:#}")).await;
        }
    };

    let backend = match backend::load_from_config(cfg) {
        Ok(b) => b,
        Err(e) => {
            return finish_with_error(w, id, "backend_init_failed", &format!("{e:#}")).await;
        }
    };
    let backend_name: &'static str = backend.name();

    let (tx, mut rx) = mpsc::channel::<BackendEvent>(32);
    let messages = vec![Message {
        role: Role::User,
        text: user_text.to_string(),
    }];
    let backend_task = tokio::spawn(async move {
        let _ = backend.stream_complete(messages, registry, tx).await;
    });

    let mut assistant_acc = String::new();
    let mut tool_records: Vec<ToolRecord> = Vec::new();
    let mut saw_error = false;

    while let Some(ev) = rx.recv().await {
        match ev {
            BackendEvent::Chunk(chunk) => {
                assistant_acc.push_str(&chunk);
                let frame = Envelope::new(ServerMessage::AssistantChunk {
                    id: id.into(),
                    text: chunk,
                });
                write_frame_async(w, &frame).await?;
            }
            BackendEvent::ToolInvoke { name } => {
                let frame = Envelope::new(ServerMessage::ToolActivity {
                    id: id.into(),
                    name,
                    phase: "invoke".into(),
                    detail: String::new(),
                });
                write_frame_async(w, &frame).await?;
            }
            BackendEvent::ToolResult {
                name,
                ok,
                latency_ms,
                detail,
            } => {
                let frame = Envelope::new(ServerMessage::ToolActivity {
                    id: id.into(),
                    name: name.clone(),
                    phase: if ok { "ok".into() } else { "error".into() },
                    detail,
                });
                write_frame_async(w, &frame).await?;
                tool_records.push(ToolRecord {
                    name,
                    ok,
                    latency_ms,
                });
            }
            BackendEvent::Error(e) => {
                let err = Envelope::new(ServerMessage::Error {
                    id: id.into(),
                    code: "backend_error".into(),
                    text: format!("{e:#}"),
                });
                write_frame_async(w, &err).await?;
                saw_error = true;
                break;
            }
        }
    }

    let _ = backend_task.await;

    let finish_reason = if saw_error { "error" } else { "stop" };
    let end = Envelope::new(ServerMessage::AssistantEnd {
        id: id.into(),
        finish_reason: finish_reason.into(),
    });
    write_frame_async(w, &end).await?;

    if let Err(e) = ledger.append(&Entry {
        ts_ms: ledger::now_ms(),
        conv_id: id,
        user: user_text,
        assistant: &assistant_acc,
        backend: backend_name,
        latency_ms: started.elapsed().as_millis(),
        finish_reason,
        tool_calls: &tool_records,
    }) {
        eprintln!("bunzod: ledger append failed: {e:#}");
    }

    Ok(())
}

async fn finish_with_error<W>(w: &mut W, id: &str, code: &str, text: &str) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let err = Envelope::new(ServerMessage::Error {
        id: id.into(),
        code: code.into(),
        text: text.into(),
    });
    write_frame_async(w, &err).await?;
    let end = Envelope::new(ServerMessage::AssistantEnd {
        id: id.into(),
        finish_reason: "error".into(),
    });
    write_frame_async(w, &end).await?;
    Ok(())
}
