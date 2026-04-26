//! bunzo wire protocol v1.
//!
//! Framing: a 4-byte big-endian unsigned length, then that many bytes of JSON.
//! Both directions use the same framing. Messages are versioned via the
//! top-level `v` field on every frame. Frame bodies are capped at 1 MiB —
//! bunzo never expects anything close to that, so the cap exists to fail fast
//! on a desynced stream.
//!
//! The `tokio` feature adds async counterparts of [`read_frame`] and
//! [`write_frame`] under [`async_io`].

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_FRAME_BYTES: u32 = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub updated_at_ms: u64,
    pub message_count: u32,
    pub last_task_status: String,
    pub last_user_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: String,
    pub conversation_id: String,
    pub task_run_id: String,
    #[serde(default = "default_task_kind")]
    pub task_kind: String,
    pub updated_at_ms: u64,
    pub task_status: String,
    pub run_status: String,
    pub summary: String,
    pub state_reason_code: Option<String>,
    pub state_reason_text: Option<String>,
    pub snapshot_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySummary {
    pub policy_id: String,
    pub subject: String,
    pub action: String,
    pub resource: String,
    pub decision: String,
    pub grant_scope: String,
    pub conversation_id: Option<String>,
    pub task_id: Option<String>,
    pub task_run_id: Option<String>,
    pub note_text: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJobSummary {
    pub job_id: String,
    pub name: String,
    #[serde(default)]
    pub prompt_text: String,
    pub prompt_preview: String,
    #[serde(default = "default_scheduled_job_trigger_kind")]
    pub trigger_kind: String,
    pub interval_seconds: u64,
    #[serde(default)]
    pub retry_max_attempts: u32,
    #[serde(default)]
    pub retry_initial_backoff_seconds: u64,
    #[serde(default)]
    pub retry_max_backoff_seconds: u64,
    pub enabled: bool,
    pub next_run_at_ms: u64,
    #[serde(default)]
    pub pending_retry_at_ms: Option<u64>,
    #[serde(default)]
    pub pending_retry_attempt: Option<u32>,
    pub conversation_id: Option<String>,
    pub last_run_status: Option<String>,
    #[serde(default)]
    pub last_run_trigger: Option<String>,
    #[serde(default)]
    pub last_run_attempt: Option<u32>,
    pub last_task_id: Option<String>,
    pub last_task_run_id: Option<String>,
    #[serde(default)]
    pub last_error_text: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningStatus {
    pub phase: String,
    pub ready: bool,
    pub device_name: Option<String>,
    pub connectivity_kind: Option<String>,
    #[serde(default)]
    pub existing_network_interface: Option<String>,
    #[serde(default)]
    pub static_ipv4_interface: Option<String>,
    #[serde(default)]
    pub static_ipv4_address: Option<String>,
    #[serde(default)]
    pub static_ipv4_prefix_len: Option<u8>,
    #[serde(default)]
    pub static_ipv4_gateway: Option<String>,
    #[serde(default)]
    pub static_ipv4_dns_servers: Vec<String>,
    pub provider_kind: Option<String>,
    pub model: Option<String>,
    pub rendered_config_path: Option<String>,
    pub secret_path: Option<String>,
    pub detail: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningSetupInput {
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub connectivity_kind: Option<String>,
    #[serde(default)]
    pub existing_network_interface: Option<String>,
    #[serde(default)]
    pub static_ipv4_interface: Option<String>,
    #[serde(default)]
    pub static_ipv4_address: Option<String>,
    #[serde(default)]
    pub static_ipv4_prefix_len: Option<u8>,
    #[serde(default)]
    pub static_ipv4_gateway: Option<String>,
    #[serde(default)]
    pub static_ipv4_dns_servers: Vec<String>,
    #[serde(default)]
    pub provider_kind: Option<String>,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    UserMessage {
        id: String,
        text: String,
        #[serde(default)]
        conversation_id: Option<String>,
    },
    Cancel {
        id: String,
    },
    ListConversations {
        id: String,
        #[serde(default = "default_list_limit")]
        limit: u32,
    },
    ListTasks {
        id: String,
        #[serde(default = "default_list_limit")]
        limit: u32,
    },
    ListPolicies {
        id: String,
        #[serde(default = "default_list_limit")]
        limit: u32,
    },
    ListScheduledJobs {
        id: String,
        #[serde(default = "default_list_limit")]
        limit: u32,
    },
    UpsertPolicy {
        id: String,
        subject: String,
        action: String,
        resource: String,
        decision: String,
        grant_scope: String,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        note_text: Option<String>,
    },
    ResolveApproval {
        id: String,
        task_run_id: String,
        grant_scope: String,
        #[serde(default)]
        note_text: Option<String>,
    },
    DeletePolicy {
        id: String,
        policy_id: String,
    },
    CreateScheduledJob {
        id: String,
        name: String,
        prompt: String,
        #[serde(default = "default_scheduled_job_trigger_kind")]
        trigger_kind: String,
        interval_seconds: u64,
        #[serde(default)]
        run_at_ms: Option<u64>,
        #[serde(default = "default_job_retry_max_attempts")]
        retry_max_attempts: u32,
        #[serde(default = "default_job_retry_initial_backoff_seconds")]
        retry_initial_backoff_seconds: u64,
        #[serde(default = "default_job_retry_max_backoff_seconds")]
        retry_max_backoff_seconds: u64,
    },
    UpdateScheduledJob {
        id: String,
        job_id: String,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        trigger_kind: Option<String>,
        #[serde(default)]
        interval_seconds: Option<u64>,
        #[serde(default)]
        run_at_ms: Option<u64>,
        #[serde(default)]
        retry_max_attempts: Option<u32>,
        #[serde(default)]
        retry_initial_backoff_seconds: Option<u64>,
        #[serde(default)]
        retry_max_backoff_seconds: Option<u64>,
    },
    DeleteScheduledJob {
        id: String,
        job_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
    RequestContext {
        id: String,
        conversation_id: String,
        task_id: String,
        task_run_id: String,
        created_conversation: bool,
    },
    AssistantChunk {
        id: String,
        text: String,
    },
    AssistantEnd {
        id: String,
        finish_reason: String,
    },
    Error {
        id: String,
        code: String,
        text: String,
    },
    /// Emitted when bunzod invokes a skill on behalf of the user. `phase` is
    /// one of `"invoke"`, `"ok"`, `"error"`. `detail` is a short human string
    /// (skill name at minimum, optionally a reason on error). Additive since
    /// v1; shells that predate it should tolerate unknown variants.
    ToolActivity {
        id: String,
        name: String,
        phase: String,
        #[serde(default)]
        detail: String,
    },
    PolicyDecision {
        id: String,
        subject: String,
        action: String,
        resource: String,
        decision: String,
        grant_scope: String,
        #[serde(default)]
        detail: String,
    },
    ConversationList {
        id: String,
        conversations: Vec<ConversationSummary>,
    },
    TaskList {
        id: String,
        tasks: Vec<TaskSummary>,
    },
    PolicyList {
        id: String,
        policies: Vec<PolicySummary>,
    },
    ScheduledJobList {
        id: String,
        jobs: Vec<ScheduledJobSummary>,
    },
    PolicyMutationResult {
        id: String,
        policy: PolicySummary,
        created: bool,
    },
    PolicyDeleteResult {
        id: String,
        policy_id: String,
    },
    ScheduledJobMutationResult {
        id: String,
        job: ScheduledJobSummary,
    },
    ScheduledJobDeleteResult {
        id: String,
        job_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProvisionClientMessage {
    GetProvisioningStatus {
        id: String,
    },
    ApplySetup {
        id: String,
        setup: ProvisioningSetupInput,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProvisionServerMessage {
    ProvisioningStatus {
        id: String,
        status: ProvisioningStatus,
    },
    ProvisioningResult {
        id: String,
        status: ProvisioningStatus,
    },
    Error {
        id: String,
        code: String,
        text: String,
    },
}

fn default_list_limit() -> u32 {
    10
}

fn default_task_kind() -> String {
    "shell_request".into()
}

fn default_scheduled_job_trigger_kind() -> String {
    "interval".into()
}

fn default_job_retry_max_attempts() -> u32 {
    2
}

fn default_job_retry_initial_backoff_seconds() -> u64 {
    30
}

fn default_job_retry_max_backoff_seconds() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub v: u8,
    #[serde(flatten)]
    pub msg: T,
}

impl<T> Envelope<T> {
    pub fn new(msg: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            msg,
        }
    }
}

pub type ClientFrame = Envelope<ClientMessage>;
pub type ServerFrame = Envelope<ServerMessage>;
pub type ProvisionClientFrame = Envelope<ProvisionClientMessage>;
pub type ProvisionServerFrame = Envelope<ProvisionServerMessage>;

pub fn write_frame<W, T>(w: &mut W, msg: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame body {} exceeds cap {}", body.len(), MAX_FRAME_BYTES),
        ));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<R, T>(r: &mut R) -> io::Result<T>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame len {} exceeds cap {}", len, MAX_FRAME_BYTES),
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

#[cfg(feature = "tokio")]
pub mod async_io {
    use super::*;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    pub async fn write_frame_async<W, T>(w: &mut W, msg: &T) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        T: Serialize,
    {
        let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
        if body.len() > MAX_FRAME_BYTES as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame body {} exceeds cap {}", body.len(), MAX_FRAME_BYTES),
            ));
        }
        w.write_all(&(body.len() as u32).to_be_bytes()).await?;
        w.write_all(&body).await?;
        w.flush().await
    }

    pub async fn read_frame_async<R, T>(r: &mut R) -> io::Result<T>
    where
        R: AsyncRead + Unpin,
        T: DeserializeOwned,
    {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf);
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame len {} exceeds cap {}", len, MAX_FRAME_BYTES),
            ));
        }
        let mut body = vec![0u8; len as usize];
        r.read_exact(&mut body).await?;
        serde_json::from_slice(&body).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_client_user_message() {
        let out = Envelope::new(ClientMessage::UserMessage {
            id: "u1".into(),
            text: "hello".into(),
            conversation_id: None,
        });
        let mut buf = Vec::new();
        write_frame(&mut buf, &out).unwrap();
        let mut cur = Cursor::new(buf);
        let back: ClientFrame = read_frame(&mut cur).unwrap();
        assert_eq!(back.v, PROTOCOL_VERSION);
        match back.msg {
            ClientMessage::UserMessage { id, text, .. } => {
                assert_eq!(id, "u1");
                assert_eq!(text, "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn roundtrip_server_frames() {
        for msg in [
            ServerMessage::RequestContext {
                id: "u1".into(),
                conversation_id: "c1".into(),
                task_id: "t1".into(),
                task_run_id: "tr1".into(),
                created_conversation: true,
            },
            ServerMessage::AssistantChunk {
                id: "u1".into(),
                text: "part ".into(),
            },
            ServerMessage::AssistantEnd {
                id: "u1".into(),
                finish_reason: "stop".into(),
            },
            ServerMessage::Error {
                id: "u1".into(),
                code: "backend_unavailable".into(),
                text: "openai returned 500".into(),
            },
            ServerMessage::ToolActivity {
                id: "u1".into(),
                name: "read-local-file".into(),
                phase: "invoke".into(),
                detail: String::new(),
            },
            ServerMessage::PolicyDecision {
                id: "u1".into(),
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: "require_approval".into(),
                grant_scope: "task".into(),
                detail: "approval required before invoking read-local-file".into(),
            },
            ServerMessage::ConversationList {
                id: "u1".into(),
                conversations: vec![ConversationSummary {
                    conversation_id: "c1".into(),
                    updated_at_ms: 1,
                    message_count: 2,
                    last_task_status: "completed".into(),
                    last_user_text: "hello".into(),
                }],
            },
            ServerMessage::TaskList {
                id: "u1".into(),
                tasks: vec![TaskSummary {
                    task_id: "t1".into(),
                    conversation_id: "c1".into(),
                    task_run_id: "tr1".into(),
                    task_kind: "shell_request".into(),
                    updated_at_ms: 1,
                    task_status: "waiting".into(),
                    run_status: "waiting".into(),
                    summary: "hello".into(),
                    state_reason_code: Some("unconfigured".into()),
                    state_reason_text: Some("missing API key".into()),
                    snapshot_kind: Some("shell_request_waiting_v1".into()),
                }],
            },
            ServerMessage::PolicyList {
                id: "u1".into(),
                policies: vec![PolicySummary {
                    policy_id: "p1".into(),
                    subject: "shell_request".into(),
                    action: "invoke_skill".into(),
                    resource: "read-local-file".into(),
                    decision: "deny".into(),
                    grant_scope: "persistent".into(),
                    conversation_id: None,
                    task_id: None,
                    task_run_id: None,
                    note_text: Some("manual smoke".into()),
                    updated_at_ms: 1,
                }],
            },
            ServerMessage::ScheduledJobList {
                id: "u1".into(),
                jobs: vec![ScheduledJobSummary {
                    job_id: "job1".into(),
                    name: "check os".into(),
                    prompt_text: "what OS is this?".into(),
                    prompt_preview: "what OS is this?".into(),
                    trigger_kind: "interval".into(),
                    interval_seconds: 60,
                    retry_max_attempts: 2,
                    retry_initial_backoff_seconds: 30,
                    retry_max_backoff_seconds: 300,
                    enabled: true,
                    next_run_at_ms: 1,
                    pending_retry_at_ms: None,
                    pending_retry_attempt: None,
                    conversation_id: Some("c1".into()),
                    last_run_status: Some("completed".into()),
                    last_run_trigger: Some("interval".into()),
                    last_run_attempt: Some(0),
                    last_task_id: Some("t1".into()),
                    last_task_run_id: Some("tr1".into()),
                    last_error_text: None,
                    updated_at_ms: 1,
                }],
            },
            ServerMessage::PolicyMutationResult {
                id: "u1".into(),
                policy: PolicySummary {
                    policy_id: "p1".into(),
                    subject: "shell_request".into(),
                    action: "invoke_skill".into(),
                    resource: "read-local-file".into(),
                    decision: "deny".into(),
                    grant_scope: "persistent".into(),
                    conversation_id: None,
                    task_id: None,
                    task_run_id: None,
                    note_text: Some("manual smoke".into()),
                    updated_at_ms: 1,
                },
                created: true,
            },
            ServerMessage::PolicyDeleteResult {
                id: "u1".into(),
                policy_id: "p1".into(),
            },
            ServerMessage::ScheduledJobMutationResult {
                id: "u1".into(),
                job: ScheduledJobSummary {
                    job_id: "job1".into(),
                    name: "check os".into(),
                    prompt_text: "what OS is this?".into(),
                    prompt_preview: "what OS is this?".into(),
                    trigger_kind: "interval".into(),
                    interval_seconds: 60,
                    retry_max_attempts: 2,
                    retry_initial_backoff_seconds: 30,
                    retry_max_backoff_seconds: 300,
                    enabled: true,
                    next_run_at_ms: 1,
                    pending_retry_at_ms: None,
                    pending_retry_attempt: None,
                    conversation_id: Some("c1".into()),
                    last_run_status: None,
                    last_run_trigger: None,
                    last_run_attempt: None,
                    last_task_id: None,
                    last_task_run_id: None,
                    last_error_text: None,
                    updated_at_ms: 1,
                },
            },
            ServerMessage::ScheduledJobDeleteResult {
                id: "u1".into(),
                job_id: "job1".into(),
            },
        ] {
            let out = Envelope::new(msg);
            let mut buf = Vec::new();
            write_frame(&mut buf, &out).unwrap();
            let mut cur = Cursor::new(buf);
            let _back: ServerFrame = read_frame(&mut cur).unwrap();
        }
    }

    #[test]
    fn roundtrip_policy_client_frames() {
        for msg in [
            ClientMessage::ListPolicies {
                id: "ctl1".into(),
                limit: 10,
            },
            ClientMessage::ListScheduledJobs {
                id: "ctl1b".into(),
                limit: 10,
            },
            ClientMessage::UpsertPolicy {
                id: "ctl2".into(),
                subject: "shell_request".into(),
                action: "invoke_skill".into(),
                resource: "read-local-file".into(),
                decision: "deny".into(),
                grant_scope: "persistent".into(),
                target: None,
                note_text: Some("manual smoke".into()),
            },
            ClientMessage::DeletePolicy {
                id: "ctl3".into(),
                policy_id: "p1".into(),
            },
            ClientMessage::ResolveApproval {
                id: "ctl4".into(),
                task_run_id: "tr1".into(),
                grant_scope: "once".into(),
                note_text: Some("approved from shell".into()),
            },
            ClientMessage::CreateScheduledJob {
                id: "ctl5".into(),
                name: "check os".into(),
                prompt: "what OS is this?".into(),
                trigger_kind: "interval".into(),
                interval_seconds: 60,
                run_at_ms: None,
                retry_max_attempts: 2,
                retry_initial_backoff_seconds: 30,
                retry_max_backoff_seconds: 300,
            },
            ClientMessage::UpdateScheduledJob {
                id: "ctl5b".into(),
                job_id: "job1".into(),
                enabled: Some(false),
                name: None,
                prompt: None,
                trigger_kind: None,
                interval_seconds: None,
                run_at_ms: None,
                retry_max_attempts: None,
                retry_initial_backoff_seconds: None,
                retry_max_backoff_seconds: None,
            },
            ClientMessage::DeleteScheduledJob {
                id: "ctl6".into(),
                job_id: "job1".into(),
            },
        ] {
            let out = Envelope::new(msg);
            let mut buf = Vec::new();
            write_frame(&mut buf, &out).unwrap();
            let mut cur = Cursor::new(buf);
            let _back: ClientFrame = read_frame(&mut cur).unwrap();
        }
    }

    #[test]
    fn roundtrip_provisioning_frames() {
        let status = ProvisioningStatus {
            phase: "ready".into(),
            ready: true,
            device_name: Some("bunzo-qemu".into()),
            connectivity_kind: Some("existing_network".into()),
            existing_network_interface: Some("eth0".into()),
            static_ipv4_interface: None,
            static_ipv4_address: None,
            static_ipv4_prefix_len: None,
            static_ipv4_gateway: None,
            static_ipv4_dns_servers: Vec::new(),
            provider_kind: Some("openai".into()),
            model: Some("gpt-5.4-mini".into()),
            rendered_config_path: Some("/etc/bunzo/bunzod.toml".into()),
            secret_path: Some("/var/lib/bunzo/secrets/openai.key".into()),
            detail: None,
            updated_at_ms: 42,
        };

        for msg in [
            ProvisionClientMessage::GetProvisioningStatus { id: "p1".into() },
            ProvisionClientMessage::ApplySetup {
                id: "p2".into(),
                setup: ProvisioningSetupInput {
                    device_name: Some("bunzo-qemu".into()),
                    connectivity_kind: Some("existing_network".into()),
                    existing_network_interface: Some("eth0".into()),
                    static_ipv4_interface: None,
                    static_ipv4_address: None,
                    static_ipv4_prefix_len: None,
                    static_ipv4_gateway: None,
                    static_ipv4_dns_servers: Vec::new(),
                    provider_kind: Some("openai".into()),
                    api_key: "sk-test".into(),
                },
            },
        ] {
            let out = Envelope::new(msg);
            let mut buf = Vec::new();
            write_frame(&mut buf, &out).unwrap();
            let mut cur = Cursor::new(buf);
            let _back: ProvisionClientFrame = read_frame(&mut cur).unwrap();
        }

        for msg in [
            ProvisionServerMessage::ProvisioningStatus {
                id: "p1".into(),
                status: status.clone(),
            },
            ProvisionServerMessage::ProvisioningResult {
                id: "p2".into(),
                status: status.clone(),
            },
            ProvisionServerMessage::Error {
                id: "p3".into(),
                code: "invalid_request".into(),
                text: "api key cannot be empty".into(),
            },
        ] {
            let out = Envelope::new(msg);
            let mut buf = Vec::new();
            write_frame(&mut buf, &out).unwrap();
            let mut cur = Cursor::new(buf);
            let _back: ProvisionServerFrame = read_frame(&mut cur).unwrap();
        }
    }

    #[test]
    fn wire_bytes_are_big_endian_length_prefix() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Envelope::new(ClientMessage::UserMessage {
                id: "x".into(),
                text: "y".into(),
                conversation_id: None,
            }),
        )
        .unwrap();
        assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    }

    #[test]
    fn oversize_len_rejected() {
        let mut buf = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
        buf.extend(std::iter::repeat(0u8).take(16));
        let mut cur = Cursor::new(buf);
        let res: io::Result<ServerFrame> = read_frame(&mut cur);
        assert!(res.is_err());
    }
}
