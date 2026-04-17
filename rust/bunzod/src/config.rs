//! Configuration loaded from /etc/bunzo/bunzod.toml.
//!
//! The file is intended to be written by the onboarding flow and is
//! re-read on every request so edits take effect without a restart.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/bunzo/bunzod.toml";

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
    Ok(cfg)
}
