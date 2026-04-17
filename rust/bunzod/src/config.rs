//! Configuration loaded from /etc/bunzo/bunzod.toml.
//!
//! The file is intended to be written by the onboarding flow and is
//! re-read on every request so edits take effect without a restart.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/bunzo/bunzod.toml";
pub const ALLOWED_OPENAI_MODELS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
];
pub const RECOMMENDED_OPENAI_MODEL: &str = "gpt-5.4-mini";

#[derive(Debug, Clone, Deserialize)]
pub struct BunzodConfig {
    pub backend: BackendConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendConfig {
    Openai(OpenAiConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiConfig {
    pub model: String,
    pub api_key_path: PathBuf,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

impl OpenAiConfig {
    pub fn validate(&self) -> Result<()> {
        if ALLOWED_OPENAI_MODELS.contains(&self.model.as_str()) {
            return Ok(());
        }
        bail!(
            "unsupported OpenAI model '{}'; bunzo is pinned to {} (recommended interactive default: {})",
            self.model,
            ALLOWED_OPENAI_MODELS.join(", "),
            RECOMMENDED_OPENAI_MODEL,
        )
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("BUNZO_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

pub fn load() -> Result<BunzodConfig> {
    let path = config_path();
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: BunzodConfig = toml::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    match &cfg.backend {
        BackendConfig::Openai(oai) => oai
            .validate()
            .with_context(|| format!("validating {}", path.display()))?,
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(model: &str) -> OpenAiConfig {
        OpenAiConfig {
            model: model.to_string(),
            api_key_path: PathBuf::from("/etc/bunzo/openai.key"),
            base_url: None,
            system_prompt: None,
        }
    }

    #[test]
    fn allows_gpt_54_family_only() {
        for model in ALLOWED_OPENAI_MODELS {
            test_cfg(model).validate().expect("model should be allowed");
        }
    }

    #[test]
    fn rejects_non_gpt_54_models() {
        let err = test_cfg("gpt-4o-mini")
            .validate()
            .expect_err("model should be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported OpenAI model 'gpt-4o-mini'"));
        assert!(msg.contains(RECOMMENDED_OPENAI_MODEL));
    }
}
