//! Skill manifest parsing.
//!
//! Every skill ships as a directory containing `manifest.toml` and
//! `skill.wasm`. The manifest carries a display name, a JSON-Schema
//! description of the skill's parameters (passed to the LLM as a tool
//! schema), and the list of capabilities the skill is allowed to exercise.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[allow(dead_code)]
    pub version: String,
    pub description: String,
    #[serde(default = "default_entry")]
    #[allow(dead_code)]
    pub entry: String,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub capabilities: Capabilities,
}

fn default_entry() -> String {
    "run".to_string()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub fs_read: Vec<String>,
}

impl Capabilities {
    /// Returns true if the skill is allowed to read the given path. Entries
    /// ending in `/` act as directory prefixes; anything else must match
    /// exactly. No path normalisation is applied beyond rejecting `..` —
    /// letting callers escape the whitelist via symlinks or `..` segments is
    /// the exact failure mode we're here to prevent.
    pub fn allows_read(&self, path: &str) -> bool {
        if path.contains("..") {
            return false;
        }
        for allowed in &self.fs_read {
            if allowed.ends_with('/') {
                if path.starts_with(allowed) {
                    return true;
                }
            } else if path == allowed {
                return true;
            }
        }
        false
    }
}

/// Load a single skill from a directory containing `manifest.toml` +
/// `skill.wasm`.
pub fn load_dir(dir: &Path) -> Result<(Manifest, PathBuf)> {
    let manifest_path = dir.join("manifest.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&raw)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let wasm_path = dir.join("skill.wasm");
    if !wasm_path.exists() {
        return Err(anyhow!(
            "skill {} missing skill.wasm at {}",
            manifest.name,
            wasm_path.display(),
        ));
    }
    Ok((manifest, wasm_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_exact_match() {
        let caps = Capabilities {
            fs_read: vec!["/etc/os-release".into()],
        };
        assert!(caps.allows_read("/etc/os-release"));
        assert!(!caps.allows_read("/etc/passwd"));
        assert!(!caps.allows_read("/etc/os-release.bak"));
    }

    #[test]
    fn capability_dir_prefix() {
        let caps = Capabilities {
            fs_read: vec!["/var/lib/bunzo/".into()],
        };
        assert!(caps.allows_read("/var/lib/bunzo/ledger.jsonl"));
        assert!(!caps.allows_read("/var/lib/bunzo"));
        assert!(!caps.allows_read("/var/lib/bunzoo/x"));
    }

    #[test]
    fn dot_dot_segments_always_denied() {
        let caps = Capabilities {
            fs_read: vec!["/etc/".into()],
        };
        assert!(!caps.allows_read("/etc/../root/.ssh/id_rsa"));
    }
}
