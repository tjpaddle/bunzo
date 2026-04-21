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
mod policy;
mod skills;
mod store;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bunzo_proto::async_io::{read_frame_async, write_frame_async};
use bunzo_proto::{ClientFrame, ClientMessage, Envelope, ServerMessage, PROTOCOL_VERSION};
use listenfd::ListenFd;
use sd_notify::NotifyState;
use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::backend::BackendEvent;
use crate::ledger::{Entry, Ledger, ToolRecord};
use crate::policy::{Decision as PolicyDecision, GrantScope, NewRuntimePolicy, ToolPolicyContext};
use crate::skills::Registry;
use crate::store::{LookupError, PrepareRequestError, RuntimeStore};

const SOCKET_PATH: &str = "/run/bunzod.sock";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let listener = acquire_listener()?;
    let ledger = Arc::new(Ledger::new(Ledger::default_path()));
    let store = Arc::new(RuntimeStore::new(RuntimeStore::default_path()));
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
        let store = Arc::clone(&store);
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, ledger, store, registry).await {
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
    store: Arc<RuntimeStore>,
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
            ClientMessage::UserMessage {
                id,
                text,
                conversation_id,
            } => {
                handle_user_message(
                    &mut write_half,
                    &id,
                    &text,
                    conversation_id.as_deref(),
                    &ledger,
                    &store,
                    registry.clone(),
                )
                .await?;
            }
            ClientMessage::ListConversations { id, limit } => {
                handle_list_conversations(&mut write_half, &id, limit, &store).await?;
            }
            ClientMessage::ListTasks { id, limit } => {
                handle_list_tasks(&mut write_half, &id, limit, &store).await?;
            }
            ClientMessage::ListPolicies { id, limit } => {
                handle_list_policies(&mut write_half, &id, limit, &store).await?;
            }
            ClientMessage::UpsertPolicy {
                id,
                subject,
                action,
                resource,
                decision,
                grant_scope,
                target,
                note_text,
            } => {
                handle_upsert_policy(
                    &mut write_half,
                    &id,
                    &subject,
                    &action,
                    &resource,
                    &decision,
                    &grant_scope,
                    target.as_deref(),
                    note_text.as_deref(),
                    &store,
                )
                .await?;
            }
            ClientMessage::DeletePolicy { id, policy_id } => {
                handle_delete_policy(&mut write_half, &id, &policy_id, &store).await?;
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
    requested_conversation: Option<&str>,
    ledger: &Ledger,
    store: &RuntimeStore,
    registry: Registry,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let started = Instant::now();
    let request = match store.prepare_shell_request(id, requested_conversation, user_text) {
        Ok(request) => request,
        Err(PrepareRequestError::ConversationNotFound(requested)) => {
            return finish_with_error(
                w,
                id,
                "conversation_not_found",
                &format!("conversation '{requested}' was not found"),
                "error",
            )
            .await;
        }
        Err(PrepareRequestError::ConversationAmbiguous(requested)) => {
            return finish_with_error(
                w,
                id,
                "conversation_ambiguous",
                &format!("conversation prefix '{requested}' matches multiple conversations"),
                "error",
            )
            .await;
        }
        Err(PrepareRequestError::Store(e)) => {
            return finish_with_error(w, id, "runtime_store_error", &format!("{e:#}"), "error")
                .await;
        }
    };
    let request_context = Envelope::new(ServerMessage::RequestContext {
        id: id.into(),
        conversation_id: request.conversation_id.clone(),
        task_id: request.task_id.clone(),
        task_run_id: request.task_run_id.clone(),
        created_conversation: request.created_conversation,
    });
    write_frame_async(w, &request_context).await?;

    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            let text = format!("{e:#}");
            persist_request_waiting(store, &request, "unconfigured", &text, None);
            return finish_with_error(w, id, "unconfigured", &text, "waiting").await;
        }
    };

    let backend = match backend::load_from_config(cfg) {
        Ok(b) => b,
        Err(e) => {
            let text = format!("{e:#}");
            persist_request_waiting(store, &request, "backend_init_failed", &text, None);
            return finish_with_error(w, id, "backend_init_failed", &text, "waiting").await;
        }
    };
    let backend_name: &'static str = backend.name();
    if let Err(e) = store.mark_shell_request_running(&request, Some(backend_name)) {
        eprintln!("bunzod: runtime-store running transition failed: {e:#}");
    }

    let (tx, mut rx) = mpsc::channel::<BackendEvent>(32);
    let messages = request.history.clone();
    let policy = ToolPolicyContext::new(store.clone(), request.clone());
    let backend_task = tokio::spawn(async move {
        backend
            .stream_complete(messages, registry, policy, tx)
            .await
    });

    let mut assistant_acc = String::new();
    let mut tool_records: Vec<ToolRecord> = Vec::new();
    let mut saw_error = false;
    let mut error_code: Option<&'static str> = None;
    let mut error_text: Option<String> = None;
    let mut waiting_policy = None;

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
            BackendEvent::PolicyDecision { evaluation } => {
                let frame = Envelope::new(ServerMessage::PolicyDecision {
                    id: id.into(),
                    subject: evaluation.subject.clone(),
                    action: evaluation.action.clone(),
                    resource: evaluation.resource.clone(),
                    decision: evaluation.decision.as_str().into(),
                    grant_scope: evaluation.grant_scope.as_str().into(),
                    detail: evaluation.detail.clone(),
                });
                write_frame_async(w, &frame).await?;
                if evaluation.decision == PolicyDecision::RequireApproval {
                    waiting_policy = Some(evaluation);
                }
            }
            BackendEvent::ToolInvoke { name } => {
                let frame = Envelope::new(ServerMessage::ToolActivity {
                    id: id.into(),
                    name: name.clone(),
                    phase: "invoke".into(),
                    detail: String::new(),
                });
                write_frame_async(w, &frame).await?;
                if let Err(e) = store.record_tool_invoke(&request, &name) {
                    eprintln!("bunzod: runtime-store tool invoke failed: {e:#}");
                }
            }
            BackendEvent::ToolResult {
                name,
                ok,
                latency_ms,
                detail,
            } => {
                if let Err(e) = store.record_tool_result(&request, &name, ok, latency_ms, &detail) {
                    eprintln!("bunzod: runtime-store tool result failed: {e:#}");
                }
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
                error_code = Some("backend_error");
                error_text = Some(format!("{e:#}"));
                break;
            }
        }
    }

    match backend_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) if !saw_error => {
            let text = format!("{e:#}");
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "backend_error".into(),
                text: text.clone(),
            });
            write_frame_async(w, &err).await?;
            saw_error = true;
            error_code = Some("backend_error");
            error_text = Some(text);
        }
        Err(e) if !saw_error => {
            let text = format!("backend task failed: {e}");
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "backend_error".into(),
                text: text.clone(),
            });
            write_frame_async(w, &err).await?;
            saw_error = true;
            error_code = Some("backend_error");
            error_text = Some(text);
        }
        _ => {}
    }

    if let Some(policy) = waiting_policy {
        let end = Envelope::new(ServerMessage::AssistantEnd {
            id: id.into(),
            finish_reason: "waiting".into(),
        });
        write_frame_async(w, &end).await?;
        persist_request_waiting(
            store,
            &request,
            "policy_approval_required",
            &policy.detail,
            Some(&assistant_acc),
        );
        return Ok(());
    }

    let finish_reason = if saw_error { "error" } else { "stop" };
    let end = Envelope::new(ServerMessage::AssistantEnd {
        id: id.into(),
        finish_reason: finish_reason.into(),
    });
    write_frame_async(w, &end).await?;

    persist_request_finish(
        store,
        &request,
        &assistant_acc,
        finish_reason,
        Some(backend_name),
        error_code,
        error_text.as_deref(),
    );

    if let Err(e) = ledger.append(&Entry {
        ts_ms: ledger::now_ms(),
        conv_id: &request.conversation_id,
        task_id: Some(&request.task_id),
        task_run_id: Some(&request.task_run_id),
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

async fn handle_list_conversations<W>(
    w: &mut W,
    id: &str,
    limit: u32,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.list_recent_conversations(limit) {
        Ok(conversations) => {
            let frame = Envelope::new(ServerMessage::ConversationList {
                id: id.into(),
                conversations,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(e) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
        }
    }
    Ok(())
}

async fn handle_list_tasks<W>(w: &mut W, id: &str, limit: u32, store: &RuntimeStore) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.list_recent_tasks(limit) {
        Ok(tasks) => {
            let frame = Envelope::new(ServerMessage::TaskList {
                id: id.into(),
                tasks,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(e) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
        }
    }
    Ok(())
}

async fn handle_list_policies<W>(
    w: &mut W,
    id: &str,
    limit: u32,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.list_runtime_policies(limit) {
        Ok(policies) => {
            let frame = Envelope::new(ServerMessage::PolicyList {
                id: id.into(),
                policies,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(e) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
        }
    }
    Ok(())
}

async fn handle_upsert_policy<W>(
    w: &mut W,
    id: &str,
    subject: &str,
    action: &str,
    resource: &str,
    decision: &str,
    grant_scope: &str,
    target: Option<&str>,
    note_text: Option<&str>,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let decision = match PolicyDecision::from_str(decision) {
        Some(decision) => decision,
        None => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "invalid_policy_decision".into(),
                text: format!("unsupported policy decision '{decision}'"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    };
    let grant_scope = match GrantScope::from_str(grant_scope) {
        Some(scope) => scope,
        None => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "invalid_policy_scope".into(),
                text: format!("unsupported policy scope '{grant_scope}'"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    };

    let (conversation_id, task_id, task_run_id) =
        match resolve_policy_target(store, grant_scope, target) {
            Ok(targets) => targets,
            Err(LookupError::NotFound { kind, value }) => {
                let err = Envelope::new(ServerMessage::Error {
                    id: id.into(),
                    code: "policy_target_not_found".into(),
                    text: format!("{kind} '{value}' was not found"),
                });
                write_frame_async(w, &err).await?;
                return Ok(());
            }
            Err(LookupError::Ambiguous { kind, value }) => {
                let err = Envelope::new(ServerMessage::Error {
                    id: id.into(),
                    code: "policy_target_ambiguous".into(),
                    text: format!("{kind} prefix '{value}' matches multiple records"),
                });
                write_frame_async(w, &err).await?;
                return Ok(());
            }
            Err(LookupError::Store(e)) => {
                let err = Envelope::new(ServerMessage::Error {
                    id: id.into(),
                    code: "runtime_store_error".into(),
                    text: format!("{e:#}"),
                });
                write_frame_async(w, &err).await?;
                return Ok(());
            }
        };

    match store.upsert_runtime_policy(NewRuntimePolicy {
        subject: subject.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        decision,
        grant_scope,
        conversation_id,
        task_id,
        task_run_id,
        note_text: note_text.map(str::to_string),
    }) {
        Ok((policy, created)) => {
            let frame = Envelope::new(ServerMessage::PolicyMutationResult {
                id: id.into(),
                policy,
                created,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(e) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
        }
    }
    Ok(())
}

async fn handle_delete_policy<W>(
    w: &mut W,
    id: &str,
    policy_id: &str,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.delete_runtime_policy(policy_id) {
        Ok(policy_id) => {
            let frame = Envelope::new(ServerMessage::PolicyDeleteResult {
                id: id.into(),
                policy_id,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(LookupError::NotFound { kind, value }) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "policy_not_found".into(),
                text: format!("{kind} '{value}' was not found"),
            });
            write_frame_async(w, &err).await?;
        }
        Err(LookupError::Ambiguous { kind, value }) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "policy_ambiguous".into(),
                text: format!("{kind} prefix '{value}' matches multiple records"),
            });
            write_frame_async(w, &err).await?;
        }
        Err(LookupError::Store(e)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
        }
    }
    Ok(())
}

fn resolve_policy_target(
    store: &RuntimeStore,
    grant_scope: GrantScope,
    target: Option<&str>,
) -> std::result::Result<(Option<String>, Option<String>, Option<String>), LookupError> {
    let target = target.map(str::trim).filter(|target| !target.is_empty());
    match grant_scope {
        GrantScope::Persistent => Ok((None, None, None)),
        GrantScope::Session => {
            let target = target.ok_or_else(|| LookupError::NotFound {
                kind: "conversation",
                value: "target required for session scope".into(),
            })?;
            Ok((Some(store.resolve_conversation_ref(target)?), None, None))
        }
        GrantScope::Task => {
            let target = target.ok_or_else(|| LookupError::NotFound {
                kind: "task",
                value: "target required for task scope".into(),
            })?;
            Ok((None, Some(store.resolve_task_ref(target)?), None))
        }
        GrantScope::Once => {
            let target = target.ok_or_else(|| LookupError::NotFound {
                kind: "task run",
                value: "target required for once scope".into(),
            })?;
            Ok((None, None, Some(store.resolve_task_run_ref(target)?)))
        }
    }
}

async fn finish_with_error<W>(
    w: &mut W,
    id: &str,
    code: &str,
    text: &str,
    finish_reason: &str,
) -> Result<()>
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
        finish_reason: finish_reason.into(),
    });
    write_frame_async(w, &end).await?;
    Ok(())
}

fn persist_request_finish(
    store: &RuntimeStore,
    request: &store::PreparedRequest,
    assistant_text: &str,
    finish_reason: &str,
    backend: Option<&str>,
    error_code: Option<&str>,
    error_text: Option<&str>,
) {
    if let Err(e) = store.finish_shell_request(
        request,
        assistant_text,
        finish_reason,
        backend,
        error_code,
        error_text,
    ) {
        eprintln!("bunzod: runtime-store finish failed: {e:#}");
    }
}

fn persist_request_waiting(
    store: &RuntimeStore,
    request: &store::PreparedRequest,
    reason_code: &str,
    reason_text: &str,
    assistant_partial_text: Option<&str>,
) {
    if let Err(e) =
        store.wait_shell_request(request, reason_code, reason_text, assistant_partial_text)
    {
        eprintln!("bunzod: runtime-store waiting transition failed: {e:#}");
    }
}
