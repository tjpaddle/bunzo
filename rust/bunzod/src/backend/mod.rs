//! Pluggable LLM backend abstraction.
//!
//! A `Backend` turns a conversation into a stream of string chunks. The
//! actual transport (Unix socket → shell) lives in `main.rs`; backends only
//! produce chunks.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

pub mod openai;

#[derive(Debug, Clone)]
pub enum Role {
    User,
    // Multi-turn history lands in M4; the variant is reserved so the backend
    // signature doesn't churn when it's wired up.
    #[allow(dead_code)]
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

#[async_trait]
pub trait Backend: Send + Sync {
    /// Stream an assistant reply as string fragments through `tx`. Returns
    /// when the stream ends cleanly; a closed `tx` means the consumer is
    /// gone and the backend should stop.
    async fn stream_complete(
        &self,
        messages: Vec<Message>,
        tx: mpsc::Sender<Result<String>>,
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
