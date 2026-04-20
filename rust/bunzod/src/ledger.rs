//! Append-only JSONL action ledger.
//!
//! One line per exchange — user prompt, assistant reply, backend, latency,
//! plus a record of any skills that ran during the exchange. Fsync after
//! every write so a crash can only ever lose an in-flight exchange, never a
//! completed one.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

pub const DEFAULT_LEDGER_PATH: &str = "/var/lib/bunzo/ledger.jsonl";

pub struct Ledger {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ToolRecord {
    pub name: String,
    pub ok: bool,
    pub latency_ms: u128,
}

#[derive(Debug, Serialize)]
pub struct Entry<'a> {
    pub ts_ms: u128,
    pub conv_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_run_id: Option<&'a str>,
    pub user: &'a str,
    pub assistant: &'a str,
    pub backend: &'a str,
    pub latency_ms: u128,
    pub finish_reason: &'a str,
    pub tool_calls: &'a [ToolRecord],
}

impl Ledger {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> PathBuf {
        std::env::var_os("BUNZO_LEDGER")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_LEDGER_PATH))
    }

    pub fn append(&self, entry: &Entry<'_>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        let mut line = serde_json::to_vec(entry).context("serializing ledger entry")?;
        line.push(b'\n');

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening {}", self.path.display()))?;
        f.write_all(&line)
            .with_context(|| format!("writing to {}", self.path.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync {}", self.path.display()))?;
        Ok(())
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
