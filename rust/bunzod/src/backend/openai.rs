//! OpenAI-compatible chat completions backend (streaming).
//!
//! Reads the API key from a file path given in the config so it never lives
//! in process env or gets logged by journald. Supports overriding `base_url`
//! so the same backend works with local OpenAI-compatible servers.

use anyhow::{bail, Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;

use super::{Backend, Message, Role};
use crate::config::OpenAiConfig;

pub struct OpenAiBackend {
    client: Client<OpenAIConfig>,
    model: String,
    system_prompt: Option<String>,
}

impl OpenAiBackend {
    pub fn new(cfg: OpenAiConfig) -> Result<Self> {
        let key = std::fs::read_to_string(&cfg.api_key_path)
            .with_context(|| format!("reading api key from {}", cfg.api_key_path.display()))?;
        let key = key.trim().to_string();
        if key.is_empty() {
            bail!("api key file {} is empty", cfg.api_key_path.display());
        }
        let mut oai_cfg = OpenAIConfig::new().with_api_key(key);
        if let Some(base) = cfg.base_url {
            oai_cfg = oai_cfg.with_api_base(base);
        }
        Ok(Self {
            client: Client::with_config(oai_cfg),
            model: cfg.model,
            system_prompt: cfg.system_prompt,
        })
    }
}

#[async_trait]
impl Backend for OpenAiBackend {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn stream_complete(
        &self,
        messages: Vec<Message>,
        tx: mpsc::Sender<Result<String>>,
    ) -> Result<()> {
        let mut oai_msgs: Vec<ChatCompletionRequestMessage> = Vec::new();

        if let Some(sys) = &self.system_prompt {
            oai_msgs.push(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(sys.clone())
                    .build()?
                    .into(),
            );
        }

        for m in messages {
            match m.role {
                Role::User => oai_msgs.push(
                    ChatCompletionRequestUserMessageArgs::default()
                        .content(m.text)
                        .build()?
                        .into(),
                ),
                Role::Assistant => {
                    // M3 has no multi-turn history yet; assistant turns are
                    // not forwarded to the model. Wired up in M4+.
                }
            }
        }

        let req = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(oai_msgs)
            .stream(true)
            .build()?;

        let mut stream = self.client.chat().create_stream(req).await?;
        while let Some(item) = stream.next().await {
            match item {
                Ok(resp) => {
                    for choice in resp.choices {
                        if let Some(content) = choice.delta.content {
                            if content.is_empty() {
                                continue;
                            }
                            if tx.send(Ok(content)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!(e))).await;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
