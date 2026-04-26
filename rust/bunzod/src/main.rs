//! bunzod — bunzo's interactive socket daemon.
//!
//! Listens on /run/bunzod.sock (or a systemd-activated socket), speaks the
//! bunzo wire protocol v1 from `bunzo-proto`, and hands prepared requests to
//! the shared runtime execution path used by both the shell and scheduler.

use anyhow::{Context, Result};
use bunzo_proto::async_io::{read_frame_async, write_frame_async};
use bunzo_proto::{ClientFrame, ClientMessage, Envelope, ServerMessage, PROTOCOL_VERSION};
use bunzod::ledger::Ledger;
use bunzod::policy::{Decision as PolicyDecision, GrantScope, NewRuntimePolicy};
use bunzod::runtime;
use bunzod::skills::{self, Registry};
use bunzod::store::{self, LookupError, PrepareRequestError, RuntimeStore, WaitingApprovalError};
use listenfd::ListenFd;
use sd_notify::NotifyState;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{UnixListener, UnixStream};

const SOCKET_PATH: &str = "/run/bunzod.sock";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    if let Err(err) = bunzod::provisioning::reconcile_runtime_state() {
        eprintln!("bunzod: provisioning reconciliation failed: {err:#}");
    }

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
            ClientMessage::ListScheduledJobs { id, limit } => {
                handle_list_scheduled_jobs(&mut write_half, &id, limit, &store).await?;
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
            ClientMessage::CreateScheduledJob {
                id,
                name,
                prompt,
                interval_seconds,
                retry_max_attempts,
                retry_initial_backoff_seconds,
                retry_max_backoff_seconds,
            } => {
                handle_create_scheduled_job(
                    &mut write_half,
                    &id,
                    &name,
                    &prompt,
                    interval_seconds,
                    retry_max_attempts,
                    retry_initial_backoff_seconds,
                    retry_max_backoff_seconds,
                    &store,
                )
                .await?;
            }
            ClientMessage::DeleteScheduledJob { id, job_id } => {
                handle_delete_scheduled_job(&mut write_half, &id, &job_id, &store).await?;
            }
            ClientMessage::ResolveApproval {
                id,
                task_run_id,
                grant_scope,
                note_text,
            } => {
                handle_resolve_approval(
                    &mut write_half,
                    &id,
                    &task_run_id,
                    &grant_scope,
                    note_text.as_deref(),
                    &ledger,
                    &store,
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
    requested_conversation: Option<&str>,
    ledger: &Ledger,
    store: &RuntimeStore,
    registry: Registry,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
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
    write_request_context(w, id, &request).await?;

    runtime::execute_prepared_request(w, id, request, ledger, store, registry).await
}

async fn handle_resolve_approval<W>(
    w: &mut W,
    id: &str,
    requested_task_run: &str,
    grant_scope: &str,
    note_text: Option<&str>,
    ledger: &Ledger,
    store: &RuntimeStore,
    registry: Registry,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let grant_scope = match GrantScope::from_str(grant_scope) {
        Some(scope) => scope,
        None => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "invalid_policy_scope".into(),
                text: format!("unsupported approval scope '{grant_scope}'"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    };

    let waiting = match store.load_waiting_approval(requested_task_run) {
        Ok(waiting) => waiting,
        Err(WaitingApprovalError::NotFound(requested)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "task_run_not_found".into(),
                text: format!("task run '{requested}' was not found"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::Ambiguous(requested)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "task_run_ambiguous".into(),
                text: format!("task run prefix '{requested}' matches multiple records"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::NotWaiting(task_run_id)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "task_run_not_waiting".into(),
                text: format!("task run '{task_run_id}' is not waiting"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::ApprovalNotRequired(task_run_id)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "approval_not_required".into(),
                text: format!(
                    "task run '{task_run_id}' is waiting for something other than approval"
                ),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::SnapshotMissing(task_run_id))
        | Err(WaitingApprovalError::SnapshotInvalid(task_run_id)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "task_run_not_resumable".into(),
                text: format!("task run '{task_run_id}' has no valid resumable snapshot"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::PolicyContextMissing(task_run_id)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "approval_context_missing".into(),
                text: format!("task run '{task_run_id}' has no approval policy context"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
        Err(WaitingApprovalError::Store(e)) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    };

    let default_note = format!(
        "approved waiting task {} to allow {} {} [{}]",
        waiting.request.task_run_id,
        waiting.action,
        waiting.resource,
        grant_scope.as_str()
    );
    let chosen_note = note_text.unwrap_or(&default_note).to_string();
    if needs_resume_override(waiting.blocking_scope, grant_scope) {
        let (conversation_id, task_id, task_run_id) =
            policy_targets_for_request(&waiting.request, waiting.blocking_scope);
        if let Err(e) = store.upsert_runtime_policy(NewRuntimePolicy {
            subject: waiting.subject.clone(),
            action: waiting.action.clone(),
            resource: waiting.resource.clone(),
            decision: PolicyDecision::Allow,
            grant_scope: waiting.blocking_scope,
            conversation_id,
            task_id,
            task_run_id,
            note_text: Some(format!("{chosen_note} (resume override)")),
        }) {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    }

    let (conversation_id, task_id, task_run_id) =
        policy_targets_for_request(&waiting.request, grant_scope);
    let (policy, created) = match store.upsert_runtime_policy(NewRuntimePolicy {
        subject: waiting.subject.clone(),
        action: waiting.action.clone(),
        resource: waiting.resource.clone(),
        decision: PolicyDecision::Allow,
        grant_scope,
        conversation_id,
        task_id,
        task_run_id,
        note_text: Some(chosen_note),
    }) {
        Ok(result) => result,
        Err(e) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "runtime_store_error".into(),
                text: format!("{e:#}"),
            });
            write_frame_async(w, &err).await?;
            return Ok(());
        }
    };

    let mutation = Envelope::new(ServerMessage::PolicyMutationResult {
        id: id.into(),
        policy,
        created,
    });
    write_frame_async(w, &mutation).await?;
    write_request_context(w, id, &waiting.request).await?;

    runtime::execute_prepared_request(w, id, waiting.request, ledger, store, registry).await
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

async fn handle_list_scheduled_jobs<W>(
    w: &mut W,
    id: &str,
    limit: u32,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.list_scheduled_jobs(limit) {
        Ok(jobs) => {
            let frame = Envelope::new(ServerMessage::ScheduledJobList {
                id: id.into(),
                jobs,
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

async fn handle_create_scheduled_job<W>(
    w: &mut W,
    id: &str,
    name: &str,
    prompt: &str,
    interval_seconds: u64,
    retry_max_attempts: u32,
    retry_initial_backoff_seconds: u64,
    retry_max_backoff_seconds: u64,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    if prompt.trim().is_empty() {
        let err = Envelope::new(ServerMessage::Error {
            id: id.into(),
            code: "invalid_job_prompt".into(),
            text: "scheduled job prompt must not be empty".into(),
        });
        write_frame_async(w, &err).await?;
        return Ok(());
    }

    match store.create_scheduled_job(bunzod::store::NewScheduledJob {
        name: name.to_string(),
        prompt: prompt.to_string(),
        interval_seconds,
        retry_max_attempts,
        retry_initial_backoff_seconds,
        retry_max_backoff_seconds,
    }) {
        Ok(job) => {
            let frame =
                Envelope::new(ServerMessage::ScheduledJobMutationResult { id: id.into(), job });
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

async fn handle_delete_scheduled_job<W>(
    w: &mut W,
    id: &str,
    job_id: &str,
    store: &RuntimeStore,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match store.delete_scheduled_job(job_id) {
        Ok(job_id) => {
            let frame = Envelope::new(ServerMessage::ScheduledJobDeleteResult {
                id: id.into(),
                job_id,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(LookupError::NotFound { kind, value }) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "job_not_found".into(),
                text: format!("{kind} '{value}' was not found"),
            });
            write_frame_async(w, &err).await?;
        }
        Err(LookupError::Ambiguous { kind, value }) => {
            let err = Envelope::new(ServerMessage::Error {
                id: id.into(),
                code: "job_ambiguous".into(),
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

fn policy_targets_for_request(
    request: &store::PreparedRequest,
    grant_scope: GrantScope,
) -> (Option<String>, Option<String>, Option<String>) {
    match grant_scope {
        GrantScope::Persistent => (None, None, None),
        GrantScope::Session => (Some(request.conversation_id.clone()), None, None),
        GrantScope::Task => (None, Some(request.task_id.clone()), None),
        GrantScope::Once => (None, None, Some(request.task_run_id.clone())),
    }
}

fn needs_resume_override(blocking_scope: GrantScope, chosen_scope: GrantScope) -> bool {
    blocking_scope.precedence() > chosen_scope.precedence()
}

async fn write_request_context<W>(
    w: &mut W,
    id: &str,
    request: &store::PreparedRequest,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let request_context = Envelope::new(ServerMessage::RequestContext {
        id: id.into(),
        conversation_id: request.conversation_id.clone(),
        task_id: request.task_id.clone(),
        task_run_id: request.task_run_id.clone(),
        created_conversation: request.created_conversation,
    });
    write_frame_async(w, &request_context).await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broader_grants_add_resume_override_for_narrower_blocking_rules() {
        assert!(needs_resume_override(
            GrantScope::Task,
            GrantScope::Persistent
        ));
        assert!(needs_resume_override(
            GrantScope::Session,
            GrantScope::Persistent
        ));
        assert!(needs_resume_override(GrantScope::Once, GrantScope::Task));
        assert!(!needs_resume_override(
            GrantScope::Persistent,
            GrantScope::Session
        ));
        assert!(!needs_resume_override(GrantScope::Task, GrantScope::Task));
    }
}
