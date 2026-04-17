//! Pluggable LLM backend abstraction.
//!
//! A `Backend` turns a conversation into a stream of events: plain text
//! chunks, tool-invocation markers, and terminal errors. The actual transport
//! (Unix socket → shell) and the ledger live in `main.rs`; backends only
//! produce events.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::skills::Registry;

pub mod openai;

#[derive(Debug, Clone)]
pub enum Role {
    User,
    // Multi-turn history lands after M4; the variant is reserved so the
    // backend signature doesn't churn when it's wired up.
    #[allow(dead_code)]
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// Events streamed out of a backend.
pub enum BackendEvent {
    /// Next chunk of user-facing assistant text.
    Chunk(String),
    /// Backend is about to invoke a skill.
    ToolInvoke { name: String },
    /// Skill invocation finished.
    ToolResult {
        name: String,
        ok: bool,
        latency_ms: u128,
        detail: String,
    },
    /// Terminal error — backend will stop after this.
    Error(anyhow::Error),
}

#[async_trait]
pub trait Backend: Send + Sync {
    /// Stream an assistant reply as events through `tx`. Returns when the
    /// stream ends cleanly; a closed `tx` means the consumer is gone and the
    /// backend should stop.
    async fn stream_complete(
        &self,
        messages: Vec<Message>,
        registry: Registry,
        tx: mpsc::Sender<BackendEvent>,
    ) -> Result<()>;

    /// Short identifier, used in the ledger and error messages.
    fn name(&self) -> &'static str;
}

use crate::config::{BackendConfig, BunzodConfig};
use openai::OpenAiBackend;

pub fn load_from_config(cfg: BunzodConfig) -> Result<Box<dyn Backend>> {
    match cfg.backend {
        BackendConfig::Openai(oai) => Ok(Box::new(OpenAiBackend::new(oai)?)),
    }
}
