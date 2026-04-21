//! Canonical runtime store for conversations, messages, tasks, task runs, and
//! internal events.
//!
//! The JSONL ledger remains the audit sink, but durable runtime state now
//! lives in a bunzo-owned SQLite database under `/var/lib/bunzo/state/`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bunzo_proto::{ConversationSummary, PolicySummary, TaskSummary};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use uuid::Uuid;

use crate::backend::{Message, Role};
use crate::policy::{
    Decision as PolicyDecision, GrantScope, NewRuntimePolicy, PolicyEvaluation, PolicySource,
};

pub const DEFAULT_STATE_DIR: &str = "/var/lib/bunzo/state";
pub const DEFAULT_DB_NAME: &str = "runtime.sqlite3";

#[derive(Debug, Clone)]
pub struct RuntimeStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    pub request_id: String,
    pub conversation_id: String,
    pub task_id: String,
    pub task_run_id: String,
    pub user_text: String,
    pub history: Vec<Message>,
    pub created_conversation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Queued,
    Running,
    Waiting,
    Completed,
    Failed,
}

impl TaskState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug)]
pub enum PrepareRequestError {
    ConversationNotFound(String),
    ConversationAmbiguous(String),
    Store(anyhow::Error),
}

impl fmt::Display for PrepareRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConversationNotFound(id) => {
                write!(f, "conversation '{id}' was not found")
            }
            Self::ConversationAmbiguous(id) => {
                write!(
                    f,
                    "conversation prefix '{id}' matches multiple conversations"
                )
            }
            Self::Store(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for PrepareRequestError {}

#[derive(Debug)]
pub enum LookupError {
    NotFound { kind: &'static str, value: String },
    Ambiguous { kind: &'static str, value: String },
    Store(anyhow::Error),
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { kind, value } => write!(f, "{kind} '{value}' was not found"),
            Self::Ambiguous { kind, value } => {
                write!(f, "{kind} prefix '{value}' matches multiple records")
            }
            Self::Store(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for LookupError {}

impl RuntimeStore {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> PathBuf {
        if let Some(path) = std::env::var_os("BUNZO_STATE_DB") {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("BUNZO_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));
        base.join(DEFAULT_DB_NAME)
    }

    pub fn prepare_shell_request(
        &self,
        request_id: &str,
        requested_conversation: Option<&str>,
        user_text: &str,
    ) -> std::result::Result<PreparedRequest, PrepareRequestError> {
        let mut conn = self.connect().map_err(PrepareRequestError::Store)?;
        let tx = conn
            .transaction()
            .map_err(anyhow::Error::from)
            .map_err(PrepareRequestError::Store)?;
        let now = now_ms_i64();

        let (conversation_id, created_conversation) = match requested_conversation
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            Some(requested) => {
                let conversation_id = resolve_conversation_id(&tx, requested)?;
                insert_event(
                    &tx,
                    &conversation_id,
                    None,
                    None,
                    "conversation.resumed",
                    json!({
                        "request_id": request_id,
                        "requested_conversation_id": requested,
                    }),
                    now,
                )
                .map_err(PrepareRequestError::Store)?;
                (conversation_id, false)
            }
            None => {
                let conversation_id = new_id();
                tx.execute(
                    concat!(
                        "INSERT INTO conversations (",
                        "id, status, created_at_ms, updated_at_ms",
                        ") VALUES (?1, ?2, ?3, ?4)"
                    ),
                    params![conversation_id, "active", now, now],
                )
                .map_err(anyhow::Error::from)
                .map_err(PrepareRequestError::Store)?;
                insert_event(
                    &tx,
                    &conversation_id,
                    None,
                    None,
                    "conversation.created",
                    json!({
                        "request_id": request_id,
                    }),
                    now,
                )
                .map_err(PrepareRequestError::Store)?;
                (conversation_id, true)
            }
        };

        tx.execute(
            "UPDATE conversations SET updated_at_ms = ?2 WHERE id = ?1",
            params![conversation_id, now],
        )
        .map_err(anyhow::Error::from)
        .map_err(PrepareRequestError::Store)?;

        let task_id = new_id();
        let task_run_id = new_id();
        let message_id = new_id();
        let summary = truncate_preview(user_text, 160);

        tx.execute(
            concat!(
                "INSERT INTO tasks (",
                "id, conversation_id, kind, status, summary, created_at_ms, updated_at_ms",
                ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            params![
                task_id,
                conversation_id,
                "shell_request",
                TaskState::Queued.as_str(),
                summary,
                now,
                now
            ],
        )
        .map_err(anyhow::Error::from)
        .map_err(PrepareRequestError::Store)?;

        tx.execute(
            concat!(
                "INSERT INTO task_runs (",
                "id, task_id, request_id, status, backend, started_at_ms, finished_at_ms, ",
                "error_code, error_text, state_reason_code, state_reason_text",
                ") VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, NULL, NULL, NULL, NULL)"
            ),
            params![
                task_run_id,
                task_id,
                request_id,
                TaskState::Queued.as_str(),
                now
            ],
        )
        .map_err(anyhow::Error::from)
        .map_err(PrepareRequestError::Store)?;

        tx.execute(
            concat!(
                "INSERT INTO messages (",
                "id, conversation_id, task_id, role, content, created_at_ms",
                ") VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
            ),
            params![message_id, conversation_id, task_id, "user", user_text, now],
        )
        .map_err(anyhow::Error::from)
        .map_err(PrepareRequestError::Store)?;

        insert_event(
            &tx,
            &conversation_id,
            Some(&task_id),
            Some(&task_run_id),
            "task.created",
            json!({
                "request_id": request_id,
                "kind": "shell_request",
                "summary": truncate_preview(user_text, 160),
            }),
            now,
        )
        .map_err(PrepareRequestError::Store)?;
        insert_event(
            &tx,
            &conversation_id,
            Some(&task_id),
            Some(&task_run_id),
            "task.queued",
            json!({
                "request_id": request_id,
                "status": TaskState::Queued.as_str(),
            }),
            now,
        )
        .map_err(PrepareRequestError::Store)?;
        insert_event(
            &tx,
            &conversation_id,
            Some(&task_id),
            Some(&task_run_id),
            "message.user",
            json!({
                "message_id": message_id,
                "chars": user_text.chars().count(),
            }),
            now,
        )
        .map_err(PrepareRequestError::Store)?;

        let history = load_history(&tx, &conversation_id).map_err(PrepareRequestError::Store)?;
        tx.commit()
            .map_err(anyhow::Error::from)
            .map_err(PrepareRequestError::Store)?;

        Ok(PreparedRequest {
            request_id: request_id.to_string(),
            conversation_id,
            task_id,
            task_run_id,
            user_text: user_text.to_string(),
            history,
            created_conversation,
        })
    }

    pub fn mark_shell_request_running(
        &self,
        request: &PreparedRequest,
        backend: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("starting running transaction")?;
        let now = now_ms_i64();

        tx.execute(
            concat!(
                "UPDATE task_runs SET ",
                "status = ?2, backend = ?3, finished_at_ms = NULL, ",
                "error_code = NULL, error_text = NULL, ",
                "state_reason_code = NULL, state_reason_text = NULL ",
                "WHERE id = ?1"
            ),
            params![request.task_run_id, TaskState::Running.as_str(), backend,],
        )
        .context("updating task run to running")?;
        tx.execute(
            "UPDATE tasks SET status = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![request.task_id, TaskState::Running.as_str(), now],
        )
        .context("updating task to running")?;
        tx.execute(
            "UPDATE conversations SET updated_at_ms = ?2 WHERE id = ?1",
            params![request.conversation_id, now],
        )
        .context("updating conversation timestamp")?;

        insert_event(
            &tx,
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            "task.run.started",
            json!({
                "backend": backend,
                "status": TaskState::Running.as_str(),
            }),
            now,
        )?;

        tx.commit().context("committing running transaction")?;
        Ok(())
    }

    pub fn wait_shell_request(
        &self,
        request: &PreparedRequest,
        reason_code: &str,
        reason_text: &str,
        assistant_partial_text: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("starting waiting transaction")?;
        let now = now_ms_i64();
        let snapshot_kind = "shell_request_waiting_v1";

        insert_snapshot(
            &tx,
            &request.task_id,
            Some(&request.task_run_id),
            snapshot_kind,
            json!({
                "request_id": request.request_id,
                "conversation_id": request.conversation_id,
                "task_id": request.task_id,
                "task_run_id": request.task_run_id,
                "resume_action": "retry_shell_request",
                "user_text": request.user_text,
                "history_message_count": request.history.len(),
                "reason_code": reason_code,
                "reason_text": reason_text,
                "assistant_partial_text": assistant_partial_text
                    .filter(|text| !text.is_empty()),
            }),
            now,
        )?;

        tx.execute(
            concat!(
                "UPDATE task_runs SET ",
                "status = ?2, backend = NULL, finished_at_ms = NULL, ",
                "error_code = NULL, error_text = NULL, ",
                "state_reason_code = ?3, state_reason_text = ?4 ",
                "WHERE id = ?1"
            ),
            params![
                request.task_run_id,
                TaskState::Waiting.as_str(),
                reason_code,
                reason_text,
            ],
        )
        .context("updating task run to waiting")?;
        tx.execute(
            "UPDATE tasks SET status = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![request.task_id, TaskState::Waiting.as_str(), now],
        )
        .context("updating task to waiting")?;
        tx.execute(
            "UPDATE conversations SET updated_at_ms = ?2 WHERE id = ?1",
            params![request.conversation_id, now],
        )
        .context("updating conversation timestamp")?;

        insert_event(
            &tx,
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            "task.waiting",
            json!({
                "reason_code": reason_code,
                "reason_text": reason_text,
                "snapshot_kind": snapshot_kind,
            }),
            now,
        )?;

        tx.commit().context("committing waiting transaction")?;
        Ok(())
    }

    pub fn record_tool_invoke(&self, request: &PreparedRequest, name: &str) -> Result<()> {
        self.record_event(
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            "tool.invoke",
            json!({ "name": name }),
        )
    }

    pub fn record_tool_result(
        &self,
        request: &PreparedRequest,
        name: &str,
        ok: bool,
        latency_ms: u128,
        detail: &str,
    ) -> Result<()> {
        self.record_event(
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            "tool.result",
            json!({
                "name": name,
                "ok": ok,
                "latency_ms": latency_ms,
                "detail": detail,
            }),
        )
    }

    pub fn finish_shell_request(
        &self,
        request: &PreparedRequest,
        assistant_text: &str,
        finish_reason: &str,
        backend: Option<&str>,
        error_code: Option<&str>,
        error_text: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("starting finish transaction")?;
        let now = now_ms_i64();
        let status = if error_code.is_some() || finish_reason == "error" {
            TaskState::Failed
        } else {
            TaskState::Completed
        };
        let state_reason_code = if status == TaskState::Failed {
            error_code
        } else {
            None
        };
        let state_reason_text = if status == TaskState::Failed {
            error_text
        } else {
            None
        };

        if !assistant_text.is_empty() {
            let message_id = new_id();
            tx.execute(
                concat!(
                    "INSERT INTO messages (",
                    "id, conversation_id, task_id, role, content, created_at_ms",
                    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                ),
                params![
                    message_id,
                    request.conversation_id,
                    request.task_id,
                    "assistant",
                    assistant_text,
                    now
                ],
            )
            .context("inserting assistant message")?;
            insert_event(
                &tx,
                &request.conversation_id,
                Some(&request.task_id),
                Some(&request.task_run_id),
                "message.assistant",
                json!({
                    "message_id": message_id,
                    "chars": assistant_text.chars().count(),
                    "finish_reason": finish_reason,
                }),
                now,
            )?;
        }

        tx.execute(
            concat!(
                "UPDATE task_runs SET ",
                "status = ?2, backend = ?3, finished_at_ms = ?4, error_code = ?5, error_text = ?6, ",
                "state_reason_code = ?7, state_reason_text = ?8 ",
                "WHERE id = ?1"
            ),
            params![
                request.task_run_id,
                status.as_str(),
                backend,
                now,
                error_code,
                error_text,
                state_reason_code,
                state_reason_text
            ],
        )
        .context("updating task run")?;
        tx.execute(
            "UPDATE tasks SET status = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![request.task_id, status.as_str(), now],
        )
        .context("updating task")?;
        tx.execute(
            "UPDATE conversations SET updated_at_ms = ?2 WHERE id = ?1",
            params![request.conversation_id, now],
        )
        .context("updating conversation")?;

        insert_event(
            &tx,
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            if status == TaskState::Completed {
                "task.completed"
            } else {
                "task.failed"
            },
            json!({
                "backend": backend,
                "finish_reason": finish_reason,
                "state_reason_code": state_reason_code,
                "state_reason_text": state_reason_text,
            }),
            now,
        )?;

        tx.commit().context("committing finish transaction")?;
        Ok(())
    }

    pub fn list_recent_conversations(&self, limit: u32) -> Result<Vec<ConversationSummary>> {
        let conn = self.connect()?;
        let capped_limit = i64::from(limit.clamp(1, 50));
        let mut stmt = conn.prepare(
            concat!(
                "SELECT ",
                "  c.id, ",
                "  c.updated_at_ms, ",
                "  (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count, ",
                "  COALESCE((SELECT t.status FROM tasks t ",
                "            WHERE t.conversation_id = c.id ",
                "            ORDER BY t.created_at_ms DESC, t.rowid DESC LIMIT 1), 'unknown') AS last_task_status, ",
                "  COALESCE((SELECT m.content FROM messages m ",
                "            WHERE m.conversation_id = c.id AND m.role = 'user' ",
                "            ORDER BY m.created_at_ms DESC, m.rowid DESC LIMIT 1), '') AS last_user_text ",
                "FROM conversations c ",
                "ORDER BY c.updated_at_ms DESC, c.rowid DESC ",
                "LIMIT ?1"
            ),
        )?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            let updated_at_ms: i64 = row.get(1)?;
            let message_count: i64 = row.get(2)?;
            let last_user_text: String = row.get(4)?;
            Ok(ConversationSummary {
                conversation_id: row.get(0)?,
                updated_at_ms: updated_at_ms.max(0) as u64,
                message_count: message_count.max(0) as u32,
                last_task_status: row.get(3)?,
                last_user_text: truncate_preview(&last_user_text, 72),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_recent_tasks(&self, limit: u32) -> Result<Vec<TaskSummary>> {
        let conn = self.connect()?;
        let capped_limit = i64::from(limit.clamp(1, 50));
        let mut stmt = conn.prepare(
            concat!(
                "SELECT ",
                "  t.id, ",
                "  t.conversation_id, ",
                "  t.updated_at_ms, ",
                "  t.status, ",
                "  t.summary, ",
                "  COALESCE((SELECT tr.id FROM task_runs tr ",
                "            WHERE tr.task_id = t.id ",
                "            ORDER BY tr.started_at_ms DESC, tr.rowid DESC LIMIT 1), '') AS task_run_id, ",
                "  COALESCE((SELECT tr.status FROM task_runs tr ",
                "            WHERE tr.task_id = t.id ",
                "            ORDER BY tr.started_at_ms DESC, tr.rowid DESC LIMIT 1), 'unknown') AS run_status, ",
                "  (SELECT tr.state_reason_code FROM task_runs tr ",
                "   WHERE tr.task_id = t.id ",
                "   ORDER BY tr.started_at_ms DESC, tr.rowid DESC LIMIT 1) AS state_reason_code, ",
                "  (SELECT tr.state_reason_text FROM task_runs tr ",
                "   WHERE tr.task_id = t.id ",
                "   ORDER BY tr.started_at_ms DESC, tr.rowid DESC LIMIT 1) AS state_reason_text, ",
                "  (SELECT ts.kind FROM task_snapshots ts ",
                "   WHERE ts.task_id = t.id ",
                "   ORDER BY ts.created_at_ms DESC, ts.rowid DESC LIMIT 1) AS snapshot_kind ",
                "FROM tasks t ",
                "ORDER BY t.updated_at_ms DESC, t.rowid DESC ",
                "LIMIT ?1"
            ),
        )?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            let updated_at_ms: i64 = row.get(2)?;
            Ok(TaskSummary {
                task_id: row.get(0)?,
                conversation_id: row.get(1)?,
                updated_at_ms: updated_at_ms.max(0) as u64,
                task_status: row.get(3)?,
                summary: row.get(4)?,
                task_run_id: row.get(5)?,
                run_status: row.get(6)?,
                state_reason_code: row.get(7)?,
                state_reason_text: row.get(8)?,
                snapshot_kind: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_runtime_policies(&self, limit: u32) -> Result<Vec<PolicySummary>> {
        let conn = self.connect()?;
        let capped_limit = i64::from(limit.clamp(1, 100));
        let mut stmt = conn.prepare(concat!(
            "SELECT id, subject, action, resource, decision, grant_scope, ",
            "conversation_id, task_id, task_run_id, note_text, updated_at_ms ",
            "FROM runtime_policies ",
            "ORDER BY updated_at_ms DESC, rowid DESC ",
            "LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            let updated_at_ms: i64 = row.get(10)?;
            Ok(PolicySummary {
                policy_id: row.get(0)?,
                subject: row.get(1)?,
                action: row.get(2)?,
                resource: row.get(3)?,
                decision: row.get(4)?,
                grant_scope: row.get(5)?,
                conversation_id: row.get(6)?,
                task_id: row.get(7)?,
                task_run_id: row.get(8)?,
                note_text: row.get(9)?,
                updated_at_ms: updated_at_ms.max(0) as u64,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_runtime_policy(&self, policy: NewRuntimePolicy) -> Result<(PolicySummary, bool)> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .context("starting runtime policy upsert transaction")?;
        let now = now_ms_i64();
        let NewRuntimePolicy {
            subject,
            action,
            resource,
            decision,
            grant_scope,
            conversation_id,
            task_id,
            task_run_id,
            note_text,
        } = policy;

        let existing_id: Option<String> = tx
            .query_row(
                concat!(
                    "SELECT id FROM runtime_policies ",
                    "WHERE subject = ?1 AND action = ?2 AND resource = ?3 AND grant_scope = ?4 ",
                    "  AND COALESCE(conversation_id, '') = COALESCE(?5, '') ",
                    "  AND COALESCE(task_id, '') = COALESCE(?6, '') ",
                    "  AND COALESCE(task_run_id, '') = COALESCE(?7, '') ",
                    "ORDER BY updated_at_ms DESC, rowid DESC LIMIT 1"
                ),
                params![
                    &subject,
                    &action,
                    &resource,
                    grant_scope.as_str(),
                    &conversation_id,
                    &task_id,
                    &task_run_id
                ],
                |row| row.get(0),
            )
            .optional()?;

        let (policy_id, created) = if let Some(policy_id) = existing_id {
            tx.execute(
                concat!(
                    "UPDATE runtime_policies SET ",
                    "decision = ?2, note_text = ?3, updated_at_ms = ?4 ",
                    "WHERE id = ?1"
                ),
                params![policy_id, decision.as_str(), &note_text, now],
            )
            .context("updating runtime policy")?;
            (policy_id, false)
        } else {
            let policy_id = new_id();
            tx.execute(
                concat!(
                    "INSERT INTO runtime_policies (",
                    "id, subject, action, resource, decision, grant_scope, ",
                    "conversation_id, task_id, task_run_id, note_text, created_at_ms, updated_at_ms",
                    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    policy_id,
                    subject,
                    action,
                    resource,
                    decision.as_str(),
                    grant_scope.as_str(),
                    conversation_id,
                    task_id,
                    task_run_id,
                    note_text,
                    now,
                    now
                ],
            )
            .context("inserting runtime policy")?;
            (policy_id, true)
        };

        let summary = load_policy_summary(&tx, &policy_id)?.context("loading runtime policy")?;
        tx.commit()
            .context("committing runtime policy upsert transaction")?;
        Ok((summary, created))
    }

    pub fn delete_runtime_policy(
        &self,
        requested_policy_id: &str,
    ) -> std::result::Result<String, LookupError> {
        let conn = self.connect().map_err(LookupError::Store)?;
        let policy_id = resolve_prefixed_id(
            &conn,
            "runtime_policies",
            "updated_at_ms",
            requested_policy_id,
            "policy",
        )?;
        conn.execute(
            "DELETE FROM runtime_policies WHERE id = ?1",
            params![policy_id],
        )
        .map_err(anyhow::Error::from)
        .map_err(LookupError::Store)?;
        Ok(policy_id)
    }

    pub fn resolve_conversation_ref(
        &self,
        requested: &str,
    ) -> std::result::Result<String, LookupError> {
        let conn = self.connect().map_err(LookupError::Store)?;
        resolve_prefixed_id(
            &conn,
            "conversations",
            "updated_at_ms",
            requested,
            "conversation",
        )
    }

    pub fn resolve_task_ref(&self, requested: &str) -> std::result::Result<String, LookupError> {
        let conn = self.connect().map_err(LookupError::Store)?;
        resolve_prefixed_id(&conn, "tasks", "updated_at_ms", requested, "task")
    }

    pub fn resolve_task_run_ref(
        &self,
        requested: &str,
    ) -> std::result::Result<String, LookupError> {
        let conn = self.connect().map_err(LookupError::Store)?;
        resolve_prefixed_id(&conn, "task_runs", "started_at_ms", requested, "task run")
    }

    pub fn insert_runtime_policy(&self, policy: NewRuntimePolicy) -> Result<String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .context("starting runtime policy transaction")?;
        let now = now_ms_i64();
        let policy_id = new_id();

        tx.execute(
            concat!(
                "INSERT INTO runtime_policies (",
                "id, subject, action, resource, decision, grant_scope, ",
                "conversation_id, task_id, task_run_id, note_text, created_at_ms, updated_at_ms",
                ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ),
            params![
                policy_id,
                policy.subject,
                policy.action,
                policy.resource,
                policy.decision.as_str(),
                policy.grant_scope.as_str(),
                policy.conversation_id,
                policy.task_id,
                policy.task_run_id,
                policy.note_text,
                now,
                now
            ],
        )
        .context("inserting runtime policy")?;

        tx.commit()
            .context("committing runtime policy transaction")?;
        Ok(policy_id)
    }

    pub fn evaluate_policy(
        &self,
        request: &PreparedRequest,
        subject: &str,
        action: &str,
        resource: &str,
    ) -> Result<PolicyEvaluation> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .context("starting runtime policy evaluation transaction")?;
        let evaluation = select_policy_evaluation(&tx, request, subject, action, resource)?;

        insert_event(
            &tx,
            &request.conversation_id,
            Some(&request.task_id),
            Some(&request.task_run_id),
            "policy.decision",
            json!({
                "policy_id": evaluation.policy_id.clone(),
                "source": evaluation.source.as_str(),
                "subject": evaluation.subject.clone(),
                "action": evaluation.action.clone(),
                "resource": evaluation.resource.clone(),
                "decision": evaluation.decision.as_str(),
                "grant_scope": evaluation.grant_scope.as_str(),
                "detail": evaluation.detail.clone(),
            }),
            now_ms_i64(),
        )?;

        tx.commit()
            .context("committing runtime policy evaluation transaction")?;
        Ok(evaluation)
    }

    fn record_event(
        &self,
        conversation_id: &str,
        task_id: Option<&str>,
        task_run_id: Option<&str>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction().context("starting event transaction")?;
        insert_event(
            &tx,
            conversation_id,
            task_id,
            task_run_id,
            kind,
            payload,
            now_ms_i64(),
        )?;
        tx.commit().context("committing event transaction")?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        ensure_parent_dir(&self.path)?;
        let conn = Connection::open(&self.path)
            .with_context(|| format!("opening runtime store {}", self.path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .context("setting sqlite busy timeout")?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("enabling sqlite foreign keys")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enabling sqlite WAL mode")?;
        ensure_schema(&conn)?;
        Ok(conn)
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    Ok(())
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(concat!(
        "CREATE TABLE IF NOT EXISTS conversations (",
        "  id TEXT PRIMARY KEY,",
        "  status TEXT NOT NULL,",
        "  created_at_ms INTEGER NOT NULL,",
        "  updated_at_ms INTEGER NOT NULL",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_conversations_updated_at ",
        "  ON conversations(updated_at_ms DESC);",
        "CREATE TABLE IF NOT EXISTS messages (",
        "  id TEXT PRIMARY KEY,",
        "  conversation_id TEXT NOT NULL,",
        "  task_id TEXT,",
        "  role TEXT NOT NULL,",
        "  content TEXT NOT NULL,",
        "  created_at_ms INTEGER NOT NULL,",
        "  FOREIGN KEY(conversation_id) REFERENCES conversations(id),",
        "  FOREIGN KEY(task_id) REFERENCES tasks(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_messages_conversation_created ",
        "  ON messages(conversation_id, created_at_ms);",
        "CREATE TABLE IF NOT EXISTS tasks (",
        "  id TEXT PRIMARY KEY,",
        "  conversation_id TEXT NOT NULL,",
        "  kind TEXT NOT NULL,",
        "  status TEXT NOT NULL,",
        "  summary TEXT NOT NULL,",
        "  created_at_ms INTEGER NOT NULL,",
        "  updated_at_ms INTEGER NOT NULL,",
        "  FOREIGN KEY(conversation_id) REFERENCES conversations(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_tasks_conversation_created ",
        "  ON tasks(conversation_id, created_at_ms);",
        "CREATE INDEX IF NOT EXISTS idx_tasks_updated_at ",
        "  ON tasks(updated_at_ms DESC);",
        "CREATE TABLE IF NOT EXISTS task_runs (",
        "  id TEXT PRIMARY KEY,",
        "  task_id TEXT NOT NULL,",
        "  request_id TEXT NOT NULL,",
        "  status TEXT NOT NULL,",
        "  backend TEXT,",
        "  started_at_ms INTEGER NOT NULL,",
        "  finished_at_ms INTEGER,",
        "  error_code TEXT,",
        "  error_text TEXT,",
        "  state_reason_code TEXT,",
        "  state_reason_text TEXT,",
        "  FOREIGN KEY(task_id) REFERENCES tasks(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_task_runs_task_started ",
        "  ON task_runs(task_id, started_at_ms);",
        "CREATE TABLE IF NOT EXISTS task_snapshots (",
        "  id TEXT PRIMARY KEY,",
        "  task_id TEXT NOT NULL,",
        "  task_run_id TEXT,",
        "  kind TEXT NOT NULL,",
        "  payload_json TEXT NOT NULL,",
        "  created_at_ms INTEGER NOT NULL,",
        "  FOREIGN KEY(task_id) REFERENCES tasks(id),",
        "  FOREIGN KEY(task_run_id) REFERENCES task_runs(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_task_snapshots_task_created ",
        "  ON task_snapshots(task_id, created_at_ms);",
        "CREATE TABLE IF NOT EXISTS runtime_policies (",
        "  id TEXT PRIMARY KEY,",
        "  subject TEXT NOT NULL,",
        "  action TEXT NOT NULL,",
        "  resource TEXT NOT NULL,",
        "  decision TEXT NOT NULL,",
        "  grant_scope TEXT NOT NULL,",
        "  conversation_id TEXT,",
        "  task_id TEXT,",
        "  task_run_id TEXT,",
        "  note_text TEXT,",
        "  created_at_ms INTEGER NOT NULL,",
        "  updated_at_ms INTEGER NOT NULL,",
        "  FOREIGN KEY(conversation_id) REFERENCES conversations(id),",
        "  FOREIGN KEY(task_id) REFERENCES tasks(id),",
        "  FOREIGN KEY(task_run_id) REFERENCES task_runs(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_runtime_policies_match ",
        "  ON runtime_policies(subject, action, resource, grant_scope, updated_at_ms DESC);",
        "CREATE INDEX IF NOT EXISTS idx_runtime_policies_task ",
        "  ON runtime_policies(task_id, task_run_id, conversation_id);",
        "CREATE TABLE IF NOT EXISTS events (",
        "  id TEXT PRIMARY KEY,",
        "  conversation_id TEXT NOT NULL,",
        "  task_id TEXT,",
        "  task_run_id TEXT,",
        "  kind TEXT NOT NULL,",
        "  payload_json TEXT NOT NULL,",
        "  created_at_ms INTEGER NOT NULL,",
        "  FOREIGN KEY(conversation_id) REFERENCES conversations(id),",
        "  FOREIGN KEY(task_id) REFERENCES tasks(id),",
        "  FOREIGN KEY(task_run_id) REFERENCES task_runs(id)",
        ");",
        "CREATE INDEX IF NOT EXISTS idx_events_conversation_created ",
        "  ON events(conversation_id, created_at_ms);",
        "CREATE INDEX IF NOT EXISTS idx_events_task_run_created ",
        "  ON events(task_run_id, created_at_ms);"
    ))
    .context("ensuring sqlite schema")?;
    ensure_column_exists(conn, "task_runs", "state_reason_code", "TEXT")?;
    ensure_column_exists(conn, "task_runs", "state_reason_text", "TEXT")?;
    Ok(())
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("preparing table_info for {table}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("querying table_info for {table}"))?;
    for row in rows {
        if row? == column {
            return Ok(());
        }
    }

    conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition};"
    ))
    .with_context(|| format!("adding column {table}.{column}"))?;
    Ok(())
}

fn load_policy_summary(tx: &Transaction<'_>, policy_id: &str) -> Result<Option<PolicySummary>> {
    tx.query_row(
        concat!(
            "SELECT id, subject, action, resource, decision, grant_scope, ",
            "conversation_id, task_id, task_run_id, note_text, updated_at_ms ",
            "FROM runtime_policies WHERE id = ?1"
        ),
        params![policy_id],
        |row| {
            let updated_at_ms: i64 = row.get(10)?;
            Ok(PolicySummary {
                policy_id: row.get(0)?,
                subject: row.get(1)?,
                action: row.get(2)?,
                resource: row.get(3)?,
                decision: row.get(4)?,
                grant_scope: row.get(5)?,
                conversation_id: row.get(6)?,
                task_id: row.get(7)?,
                task_run_id: row.get(8)?,
                note_text: row.get(9)?,
                updated_at_ms: updated_at_ms.max(0) as u64,
            })
        },
    )
    .optional()
    .map_err(anyhow::Error::from)
}

fn resolve_prefixed_id(
    conn: &Connection,
    table: &str,
    order_column: &str,
    requested: &str,
    kind: &'static str,
) -> std::result::Result<String, LookupError> {
    if let Some(exact) = conn
        .query_row(
            &format!("SELECT id FROM {table} WHERE id = ?1"),
            params![requested],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(anyhow::Error::from)
        .map_err(LookupError::Store)?
    {
        return Ok(exact);
    }

    let mut stmt = conn
        .prepare(&format!(
            "SELECT id FROM {table} WHERE id LIKE ?1 || '%' ORDER BY {order_column} DESC, rowid DESC LIMIT 2"
        ))
        .map_err(anyhow::Error::from)
        .map_err(LookupError::Store)?;
    let rows = stmt
        .query_map(params![requested], |row| row.get::<_, String>(0))
        .map_err(anyhow::Error::from)
        .map_err(LookupError::Store)?;
    let mut matches = Vec::new();
    for row in rows {
        matches.push(
            row.map_err(anyhow::Error::from)
                .map_err(LookupError::Store)?,
        );
    }

    match matches.len() {
        0 => Err(LookupError::NotFound {
            kind,
            value: requested.to_string(),
        }),
        1 => Ok(matches.pop().unwrap()),
        _ => Err(LookupError::Ambiguous {
            kind,
            value: requested.to_string(),
        }),
    }
}

fn select_policy_evaluation(
    tx: &Transaction<'_>,
    request: &PreparedRequest,
    subject: &str,
    action: &str,
    resource: &str,
) -> Result<PolicyEvaluation> {
    let matched = tx
        .query_row(
            concat!(
                "SELECT id, decision, grant_scope, note_text ",
                "FROM runtime_policies ",
                "WHERE (subject = ?1 OR subject = '*') ",
                "  AND (action = ?2 OR action = '*') ",
                "  AND (resource = ?3 OR resource = '*') ",
                "  AND (",
                "    (grant_scope = 'once' AND task_run_id = ?4) OR ",
                "    (grant_scope = 'task' AND task_id = ?5) OR ",
                "    (grant_scope = 'session' AND conversation_id = ?6) OR ",
                "    (grant_scope = 'persistent' ",
                "       AND conversation_id IS NULL ",
                "       AND task_id IS NULL ",
                "       AND task_run_id IS NULL)",
                "  ) ",
                "ORDER BY ",
                "  CASE grant_scope ",
                "    WHEN 'once' THEN 4 ",
                "    WHEN 'task' THEN 3 ",
                "    WHEN 'session' THEN 2 ",
                "    WHEN 'persistent' THEN 1 ",
                "    ELSE 0 END DESC, ",
                "  CASE WHEN resource = ?3 THEN 2 WHEN resource = '*' THEN 1 ELSE 0 END DESC, ",
                "  CASE WHEN action = ?2 THEN 2 WHEN action = '*' THEN 1 ELSE 0 END DESC, ",
                "  CASE WHEN subject = ?1 THEN 2 WHEN subject = '*' THEN 1 ELSE 0 END DESC, ",
                "  updated_at_ms DESC, rowid DESC ",
                "LIMIT 1"
            ),
            params![
                subject,
                action,
                resource,
                &request.task_run_id,
                &request.task_id,
                &request.conversation_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .context("querying runtime policy")?;

    if let Some((policy_id, decision, grant_scope, note_text)) = matched {
        let decision = PolicyDecision::from_str(&decision)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime policy decision: {decision}"))?;
        let grant_scope = GrantScope::from_str(&grant_scope)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime policy scope: {grant_scope}"))?;
        return Ok(PolicyEvaluation {
            policy_id: Some(policy_id),
            source: PolicySource::Rule,
            subject: subject.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            decision,
            grant_scope,
            detail: note_text
                .unwrap_or_else(|| default_policy_detail(decision, grant_scope, action, resource)),
        });
    }

    Ok(PolicyEvaluation {
        policy_id: None,
        source: PolicySource::Default,
        subject: subject.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        decision: PolicyDecision::Allow,
        grant_scope: GrantScope::Once,
        detail: default_policy_detail(PolicyDecision::Allow, GrantScope::Once, action, resource),
    })
}

fn default_policy_detail(
    decision: PolicyDecision,
    grant_scope: GrantScope,
    action: &str,
    resource: &str,
) -> String {
    match decision {
        PolicyDecision::Allow => {
            format!("allowed by the current default runtime policy for this {action} on {resource}")
        }
        PolicyDecision::Deny => {
            format!(
                "runtime policy denied {action} on {resource} [{}]",
                grant_scope.as_str()
            )
        }
        PolicyDecision::RequireApproval => format!(
            "approval required before bunzo may {action} on {resource} [{}]",
            grant_scope.as_str()
        ),
    }
}

fn resolve_conversation_id(
    tx: &Transaction<'_>,
    requested: &str,
) -> std::result::Result<String, PrepareRequestError> {
    match resolve_prefixed_id(
        tx,
        "conversations",
        "updated_at_ms",
        requested,
        "conversation",
    ) {
        Ok(id) => Ok(id),
        Err(LookupError::NotFound { value, .. }) => {
            Err(PrepareRequestError::ConversationNotFound(value))
        }
        Err(LookupError::Ambiguous { value, .. }) => {
            Err(PrepareRequestError::ConversationAmbiguous(value))
        }
        Err(LookupError::Store(err)) => Err(PrepareRequestError::Store(err)),
    }
}

fn load_history(tx: &Transaction<'_>, conversation_id: &str) -> Result<Vec<Message>> {
    let mut stmt = tx.prepare(concat!(
        "SELECT role, content FROM messages ",
        "WHERE conversation_id = ?1 ",
        "ORDER BY created_at_ms ASC, rowid ASC"
    ))?;
    let rows = stmt.query_map(params![conversation_id], |row| {
        let role: String = row.get(0)?;
        let text: String = row.get(1)?;
        Ok((role, text))
    })?;

    let mut history = Vec::new();
    for row in rows {
        let (role, text) = row?;
        let role = match role.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };
        history.push(Message { role, text });
    }
    Ok(history)
}

fn insert_event(
    tx: &Transaction<'_>,
    conversation_id: &str,
    task_id: Option<&str>,
    task_run_id: Option<&str>,
    kind: &str,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    tx.execute(
        concat!(
            "INSERT INTO events (",
            "id, conversation_id, task_id, task_run_id, kind, payload_json, created_at_ms",
            ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ),
        params![
            new_id(),
            conversation_id,
            task_id,
            task_run_id,
            kind,
            serde_json::to_string(&payload).context("serializing event payload")?,
            created_at_ms
        ],
    )
    .with_context(|| format!("inserting event {kind}"))?;
    Ok(())
}

fn insert_snapshot(
    tx: &Transaction<'_>,
    task_id: &str,
    task_run_id: Option<&str>,
    kind: &str,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    tx.execute(
        concat!(
            "INSERT INTO task_snapshots (",
            "id, task_id, task_run_id, kind, payload_json, created_at_ms",
            ") VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        ),
        params![
            new_id(),
            task_id,
            task_run_id,
            kind,
            serde_json::to_string(&payload).context("serializing snapshot payload")?,
            created_at_ms
        ],
    )
    .with_context(|| format!("inserting snapshot {kind}"))?;
    Ok(())
}

fn now_ms_i64() -> i64 {
    let now = crate::ledger::now_ms();
    i64::try_from(now).unwrap_or(i64::MAX)
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (idx, ch) in trimmed.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, RuntimeStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RuntimeStore::new(dir.path().join("runtime.sqlite3"));
        (dir, store)
    }

    #[test]
    fn resumed_conversation_loads_prior_messages() {
        let (_dir, store) = temp_store();

        let first = store
            .prepare_shell_request("u1", None, "hello bunzo")
            .expect("first request");
        assert!(first.created_conversation);
        assert_eq!(first.history.len(), 1);
        assert!(matches!(first.history[0].role, Role::User));
        store
            .finish_shell_request(&first, "hello back", "stop", Some("openai"), None, None)
            .expect("finish first request");

        let prefix = &first.conversation_id[..8];
        let resumed = store
            .prepare_shell_request("u2", Some(prefix), "continue")
            .expect("resume request");
        assert!(!resumed.created_conversation);
        assert_eq!(resumed.conversation_id, first.conversation_id);
        assert_eq!(resumed.history.len(), 3);
        assert!(matches!(resumed.history[0].role, Role::User));
        assert!(matches!(resumed.history[1].role, Role::Assistant));
        assert!(matches!(resumed.history[2].role, Role::User));
    }

    #[test]
    fn recent_conversations_include_latest_preview() {
        let (_dir, store) = temp_store();

        let request = store
            .prepare_shell_request("u1", None, "what OS is this?")
            .expect("request");
        store
            .finish_shell_request(&request, "bunzo 0.0.1", "stop", Some("openai"), None, None)
            .expect("finish request");

        let recent = store
            .list_recent_conversations(10)
            .expect("list conversations");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].conversation_id, request.conversation_id);
        assert_eq!(recent[0].message_count, 2);
        assert_eq!(recent[0].last_task_status, "completed");
        assert_eq!(recent[0].last_user_text, "what OS is this?");
    }

    #[test]
    fn waiting_tasks_capture_reason_and_snapshot() {
        let (_dir, store) = temp_store();

        let request = store
            .prepare_shell_request("u1", None, "finish setup")
            .expect("request");
        let queued = store.list_recent_tasks(10).expect("queued tasks");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].task_status, "queued");
        assert_eq!(queued[0].run_status, "queued");
        assert!(queued[0].snapshot_kind.is_none());

        store
            .wait_shell_request(
                &request,
                "unconfigured",
                "OpenAI backend config is missing.",
                None,
            )
            .expect("waiting request");

        let tasks = store.list_recent_tasks(10).expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, request.task_id);
        assert_eq!(tasks[0].task_status, "waiting");
        assert_eq!(tasks[0].run_status, "waiting");
        assert_eq!(tasks[0].state_reason_code.as_deref(), Some("unconfigured"));
        assert_eq!(
            tasks[0].state_reason_text.as_deref(),
            Some("OpenAI backend config is missing.")
        );
        assert_eq!(
            tasks[0].snapshot_kind.as_deref(),
            Some("shell_request_waiting_v1")
        );
    }

    #[test]
    fn running_and_completed_task_states_are_queryable() {
        let (_dir, store) = temp_store();

        let request = store
            .prepare_shell_request("u1", None, "what time is it?")
            .expect("request");
        store
            .mark_shell_request_running(&request, Some("openai"))
            .expect("mark running");

        let running = store.list_recent_tasks(10).expect("running tasks");
        assert_eq!(running[0].task_status, "running");
        assert_eq!(running[0].run_status, "running");

        store
            .finish_shell_request(&request, "it's time", "stop", Some("openai"), None, None)
            .expect("finish request");

        let completed = store.list_recent_tasks(10).expect("completed tasks");
        assert_eq!(completed[0].task_status, "completed");
        assert_eq!(completed[0].run_status, "completed");
        assert!(completed[0].state_reason_code.is_none());
    }

    #[test]
    fn runtime_policy_defaults_to_once_allow_without_matching_rule() {
        let (_dir, store) = temp_store();
        let request = store
            .prepare_shell_request("u1", None, "what OS is this?")
            .expect("request");

        let evaluation = store
            .evaluate_policy(&request, "shell_request", "invoke_skill", "read-local-file")
            .expect("policy evaluation");

        assert_eq!(evaluation.source, PolicySource::Default);
        assert_eq!(evaluation.decision, PolicyDecision::Allow);
        assert_eq!(evaluation.grant_scope, GrantScope::Once);
    }

    #[test]
    fn runtime_policy_prefers_task_scoped_rule_and_records_task_event() {
        let (_dir, store) = temp_store();
        let request = store
            .prepare_shell_request("u1", None, "what OS is this?")
            .expect("request");

        store
            .insert_runtime_policy(NewRuntimePolicy {
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: PolicyDecision::Allow,
                grant_scope: GrantScope::Persistent,
                conversation_id: None,
                task_id: None,
                task_run_id: None,
                note_text: Some("persistent allow".into()),
            })
            .expect("persistent policy");
        let deny_id = store
            .insert_runtime_policy(NewRuntimePolicy {
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: PolicyDecision::Deny,
                grant_scope: GrantScope::Task,
                conversation_id: Some(request.conversation_id.clone()),
                task_id: Some(request.task_id.clone()),
                task_run_id: None,
                note_text: Some("task-scoped deny".into()),
            })
            .expect("task policy");

        let evaluation = store
            .evaluate_policy(&request, "shell_request", "invoke_skill", "read-local-file")
            .expect("policy evaluation");
        assert_eq!(evaluation.policy_id.as_deref(), Some(deny_id.as_str()));
        assert_eq!(evaluation.source, PolicySource::Rule);
        assert_eq!(evaluation.decision, PolicyDecision::Deny);
        assert_eq!(evaluation.grant_scope, GrantScope::Task);
        assert_eq!(evaluation.detail, "task-scoped deny");

        let conn = store.connect().expect("connect");
        let (task_id, task_run_id, payload_json): (String, String, String) = conn
            .query_row(
                concat!(
                    "SELECT task_id, task_run_id, payload_json FROM events ",
                    "WHERE kind = 'policy.decision' ",
                    "ORDER BY created_at_ms DESC, rowid DESC LIMIT 1"
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("policy decision event");
        assert_eq!(task_id, request.task_id);
        assert_eq!(task_run_id, request.task_run_id);

        let payload: serde_json::Value =
            serde_json::from_str(&payload_json).expect("event payload json");
        assert_eq!(payload["policy_id"], deny_id);
        assert_eq!(payload["decision"], "deny");
        assert_eq!(payload["grant_scope"], "task");
        assert_eq!(payload["resource"], "read-local-file");
    }

    #[test]
    fn upsert_list_and_delete_runtime_policy_roundtrip() {
        let (_dir, store) = temp_store();

        let (created, was_created) = store
            .upsert_runtime_policy(NewRuntimePolicy {
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: PolicyDecision::Deny,
                grant_scope: GrantScope::Persistent,
                conversation_id: None,
                task_id: None,
                task_run_id: None,
                note_text: Some("initial".into()),
            })
            .expect("create policy");
        assert!(was_created);
        assert_eq!(created.decision, "deny");
        assert_eq!(created.note_text.as_deref(), Some("initial"));

        let (updated, was_created) = store
            .upsert_runtime_policy(NewRuntimePolicy {
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: PolicyDecision::RequireApproval,
                grant_scope: GrantScope::Persistent,
                conversation_id: None,
                task_id: None,
                task_run_id: None,
                note_text: Some("updated".into()),
            })
            .expect("update policy");
        assert!(!was_created);
        assert_eq!(updated.policy_id, created.policy_id);
        assert_eq!(updated.decision, "require_approval");

        let listed = store.list_runtime_policies(10).expect("list policies");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].policy_id, created.policy_id);
        assert_eq!(listed[0].decision, "require_approval");

        let deleted = store
            .delete_runtime_policy(&created.policy_id[..8])
            .expect("delete policy");
        assert_eq!(deleted, created.policy_id);
        assert!(store
            .list_runtime_policies(10)
            .expect("list empty")
            .is_empty());
    }
}
