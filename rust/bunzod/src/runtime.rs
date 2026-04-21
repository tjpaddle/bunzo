use std::time::Instant;

use anyhow::Result;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

use crate::backend::{self, BackendEvent};
use crate::ledger::{self, Entry, Ledger, ToolRecord};
use crate::policy::{Decision as PolicyDecision, ToolPolicyContext};
use crate::skills::Registry;
use crate::store::{PreparedRequest, RuntimeStore};

pub async fn execute_prepared_request<W>(
    w: &mut W,
    id: &str,
    request: PreparedRequest,
    ledger: &Ledger,
    store: &RuntimeStore,
    registry: Registry,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let started = Instant::now();

    let cfg = match crate::config::load() {
        Ok(c) => c,
        Err(e) => {
            let text = format!("{e:#}");
            persist_request_waiting(store, &request, "unconfigured", &text, None);
            let err = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::Error {
                id: id.into(),
                code: "unconfigured".into(),
                text,
            });
            bunzo_proto::async_io::write_frame_async(w, &err).await?;
            let end = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::AssistantEnd {
                id: id.into(),
                finish_reason: "waiting".into(),
            });
            bunzo_proto::async_io::write_frame_async(w, &end).await?;
            return Ok(());
        }
    };

    let backend = match backend::load_from_config(cfg) {
        Ok(b) => b,
        Err(e) => {
            let text = format!("{e:#}");
            persist_request_waiting(store, &request, "backend_init_failed", &text, None);
            let err = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::Error {
                id: id.into(),
                code: "backend_init_failed".into(),
                text,
            });
            bunzo_proto::async_io::write_frame_async(w, &err).await?;
            let end = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::AssistantEnd {
                id: id.into(),
                finish_reason: "waiting".into(),
            });
            bunzo_proto::async_io::write_frame_async(w, &end).await?;
            return Ok(());
        }
    };
    let backend_name: &'static str = backend.name();
    if let Err(e) = store.mark_request_running(&request, Some(backend_name)) {
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
                let frame =
                    bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::AssistantChunk {
                        id: id.into(),
                        text: chunk,
                    });
                bunzo_proto::async_io::write_frame_async(w, &frame).await?;
            }
            BackendEvent::PolicyDecision { evaluation } => {
                let frame =
                    bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::PolicyDecision {
                        id: id.into(),
                        subject: evaluation.subject.clone(),
                        action: evaluation.action.clone(),
                        resource: evaluation.resource.clone(),
                        decision: evaluation.decision.as_str().into(),
                        grant_scope: evaluation.grant_scope.as_str().into(),
                        detail: evaluation.detail.clone(),
                    });
                bunzo_proto::async_io::write_frame_async(w, &frame).await?;
                if evaluation.decision == PolicyDecision::RequireApproval {
                    waiting_policy = Some(evaluation);
                }
            }
            BackendEvent::ToolInvoke { name } => {
                let frame = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::ToolActivity {
                    id: id.into(),
                    name: name.clone(),
                    phase: "invoke".into(),
                    detail: String::new(),
                });
                bunzo_proto::async_io::write_frame_async(w, &frame).await?;
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
                let frame = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::ToolActivity {
                    id: id.into(),
                    name: name.clone(),
                    phase: if ok { "ok".into() } else { "error".into() },
                    detail,
                });
                bunzo_proto::async_io::write_frame_async(w, &frame).await?;
                tool_records.push(ToolRecord {
                    name,
                    ok,
                    latency_ms,
                });
            }
            BackendEvent::Error(e) => {
                let err = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::Error {
                    id: id.into(),
                    code: "backend_error".into(),
                    text: format!("{e:#}"),
                });
                bunzo_proto::async_io::write_frame_async(w, &err).await?;
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
            let err = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::Error {
                id: id.into(),
                code: "backend_error".into(),
                text: text.clone(),
            });
            bunzo_proto::async_io::write_frame_async(w, &err).await?;
            saw_error = true;
            error_code = Some("backend_error");
            error_text = Some(text);
        }
        Err(e) if !saw_error => {
            let text = format!("backend task failed: {e}");
            let err = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::Error {
                id: id.into(),
                code: "backend_error".into(),
                text: text.clone(),
            });
            bunzo_proto::async_io::write_frame_async(w, &err).await?;
            saw_error = true;
            error_code = Some("backend_error");
            error_text = Some(text);
        }
        _ => {}
    }

    if let Some(policy) = waiting_policy {
        let end = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::AssistantEnd {
            id: id.into(),
            finish_reason: "waiting".into(),
        });
        bunzo_proto::async_io::write_frame_async(w, &end).await?;
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
    let end = bunzo_proto::Envelope::new(bunzo_proto::ServerMessage::AssistantEnd {
        id: id.into(),
        finish_reason: finish_reason.into(),
    });
    bunzo_proto::async_io::write_frame_async(w, &end).await?;

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
        user: &request.user_text,
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

fn persist_request_finish(
    store: &RuntimeStore,
    request: &PreparedRequest,
    assistant_text: &str,
    finish_reason: &str,
    backend: Option<&str>,
    error_code: Option<&str>,
    error_text: Option<&str>,
) {
    if let Err(e) = store.finish_request(
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
    request: &PreparedRequest,
    reason_code: &str,
    reason_text: &str,
    assistant_partial_text: Option<&str>,
) {
    if let Err(e) = store.wait_request(request, reason_code, reason_text, assistant_partial_text) {
        eprintln!("bunzod: runtime-store waiting transition failed: {e:#}");
    }
}
