//! OpenAI-compatible chat completions backend with streaming + tool calling.
//!
//! Reads the API key from a file path given in the config so it never lives
//! in process env or gets logged by journald. Skills in the registry are
//! exposed to the model as OpenAI "tools"; when the model requests one, the
//! backend invokes it through [`Registry::invoke_sync`] (on a blocking task)
//! and feeds the result back to the model.

use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
        ChatCompletionTool, ChatCompletionToolArgs, ChatCompletionToolType,
        CreateChatCompletionRequestArgs, FunctionCall, FunctionObjectArgs,
    },
    Client,
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;

use super::{Backend, BackendEvent, Message, Role};
use crate::config::OpenAiConfig;
use crate::skills::Registry;

const MAX_TOOL_HOPS: u32 = 3;
const DEFAULT_SYSTEM_PROMPT: &str = concat!(
    "You are bunzo, the assistant running on the current Linux device. ",
    "When the user asks about device-local facts such as the OS/version, ",
    "hostname, message of the day, uptime, load, memory usage, files, or ",
    "bunzo ledger contents, use the available tools instead of guessing. ",
    "If a suitable tool is unavailable or access is denied, say that plainly. ",
    "Do not invent device-local values such as the current date or time."
);

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
            system_prompt: Some(
                cfg.system_prompt
                    .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
            ),
        })
    }

    fn build_tools(&self, registry: &Registry) -> Result<Vec<ChatCompletionTool>> {
        registry
            .tool_descriptors()
            .into_iter()
            .map(|t| {
                let function = FunctionObjectArgs::default()
                    .name(t.name)
                    .description(t.description)
                    .parameters(t.parameters)
                    .build()?;
                Ok(ChatCompletionToolArgs::default()
                    .r#type(ChatCompletionToolType::Function)
                    .function(function)
                    .build()?)
            })
            .collect()
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
        registry: Registry,
        tx: mpsc::Sender<BackendEvent>,
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
                Role::Assistant => oai_msgs.push(
                    ChatCompletionRequestAssistantMessageArgs::default()
                        .content(m.text)
                        .build()?
                        .into(),
                ),
            }
        }

        let tools = if registry.is_empty() {
            None
        } else {
            Some(self.build_tools(&registry)?)
        };

        for hop in 0..=MAX_TOOL_HOPS {
            let mut builder = CreateChatCompletionRequestArgs::default();
            builder
                .model(&self.model)
                .messages(oai_msgs.clone())
                .stream(true);
            if let Some(t) = &tools {
                builder.tools(t.clone());
            }
            let req = builder.build()?;

            let mut stream = self.client.chat().create_stream(req).await?;

            let mut pending_tool_calls: Vec<ToolCallAccum> = Vec::new();
            let mut finish_reason: Option<String> = None;

            while let Some(item) = stream.next().await {
                match item {
                    Ok(resp) => {
                        for choice in resp.choices {
                            if let Some(content) = choice.delta.content {
                                if !content.is_empty()
                                    && tx.send(BackendEvent::Chunk(content)).await.is_err()
                                {
                                    return Ok(());
                                }
                            }
                            if let Some(tool_chunks) = choice.delta.tool_calls {
                                for chunk in tool_chunks {
                                    accumulate_tool_call(&mut pending_tool_calls, chunk);
                                }
                            }
                            if let Some(reason) = choice.finish_reason {
                                finish_reason = Some(format!("{reason:?}").to_lowercase());
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(BackendEvent::Error(anyhow!(e))).await;
                        return Ok(());
                    }
                }
            }

            // Only treat the stream as ending in tools if the model actually
            // produced any. Some providers emit `tool_calls` as a finish_reason
            // even when the delta list is empty; guard against that.
            if pending_tool_calls.is_empty() {
                return Ok(());
            }

            if hop == MAX_TOOL_HOPS {
                let _ = tx
                    .send(BackendEvent::Error(anyhow!(
                        "tool-call hop limit ({MAX_TOOL_HOPS}) reached"
                    )))
                    .await;
                return Ok(());
            }

            // Stitch the completed assistant turn back into the history so
            // the next request sees it, then execute each tool.
            let assistant_tool_calls: Vec<ChatCompletionMessageToolCall> = pending_tool_calls
                .iter()
                .map(|t| ChatCompletionMessageToolCall {
                    id: t.id.clone(),
                    r#type: ChatCompletionToolType::Function,
                    function: FunctionCall {
                        name: t.name.clone(),
                        arguments: t.arguments.clone(),
                    },
                })
                .collect();
            let assistant_msg = ChatCompletionRequestAssistantMessageArgs::default()
                .tool_calls(assistant_tool_calls)
                .build()?;
            oai_msgs.push(assistant_msg.into());

            for call in &pending_tool_calls {
                if tx
                    .send(BackendEvent::ToolInvoke {
                        name: call.name.clone(),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }

                let started = Instant::now();
                let registry = registry.clone();
                let name = call.name.clone();
                let args_json = call.arguments.clone();
                let join =
                    tokio::task::spawn_blocking(move || registry.invoke_sync(&name, &args_json))
                        .await;

                let (tool_output, ok, detail): (String, bool, String) = match join {
                    Ok(Ok(out)) => (out, true, String::new()),
                    Ok(Err(e)) => {
                        let msg = format!("{e:#}");
                        (
                            format!("{{\"error\":\"{}\"}}", escape_json(&msg)),
                            false,
                            msg,
                        )
                    }
                    Err(e) => {
                        let msg = format!("skill task panicked: {e}");
                        (
                            format!("{{\"error\":\"{}\"}}", escape_json(&msg)),
                            false,
                            msg,
                        )
                    }
                };

                let latency_ms = started.elapsed().as_millis();
                if tx
                    .send(BackendEvent::ToolResult {
                        name: call.name.clone(),
                        ok,
                        latency_ms,
                        detail: detail.clone(),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }

                let tool_msg = ChatCompletionRequestToolMessageArgs::default()
                    .content(tool_output)
                    .tool_call_id(call.id.clone())
                    .build()?;
                oai_msgs.push(tool_msg.into());
            }

            // Ignore finish_reason; whatever it was, we have tool results now
            // and we loop to ask the model to continue.
            let _ = finish_reason;
        }

        Ok(())
    }
}

/// Accumulator for streamed tool-call fragments. OpenAI's streaming API
/// splits `tool_calls` across many delta chunks keyed by `index`; we stitch
/// them back together.
struct ToolCallAccum {
    #[allow(dead_code)]
    index: u32,
    id: String,
    name: String,
    arguments: String,
}

fn accumulate_tool_call(
    acc: &mut Vec<ToolCallAccum>,
    chunk: async_openai::types::ChatCompletionMessageToolCallChunk,
) {
    let idx = chunk.index;
    let entry = match acc.iter_mut().find(|a| a.index == idx) {
        Some(e) => e,
        None => {
            acc.push(ToolCallAccum {
                index: idx,
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
            acc.last_mut().unwrap()
        }
    };
    if let Some(id) = chunk.id {
        entry.id = id;
    }
    if let Some(func) = chunk.function {
        if let Some(name) = func.name {
            entry.name.push_str(&name);
        }
        if let Some(args) = func.arguments {
            entry.arguments.push_str(&args);
        }
    }
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
