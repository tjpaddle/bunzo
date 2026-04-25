//! Skill registry — loads all skills at bunzod startup and exposes them to
//! the backend for tool invocation.
//!
//! Skills live in `/usr/lib/bunzo/skills/<name>/{manifest.toml,skill.wasm}`.
//! The registry is cheap to clone (internally `Arc`), so it can be handed to
//! any number of in-flight backend requests.

mod host;
mod manifest;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::policy::{read_local_file_path, READ_LOCAL_FILE_SKILL};

pub use host::SkillHost;
pub use manifest::Manifest;

pub const DEFAULT_SKILLS_DIR: &str = "/usr/lib/bunzo/skills";

/// A single loaded skill — manifest + compiled wasmtime module.
pub struct LoadedSkill {
    pub manifest: Manifest,
    #[allow(dead_code)]
    pub path: PathBuf,
    skill: host::Skill,
}

/// Description of one skill, shaped for the LLM's tool-call interface.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    host: SkillHost,
    skills: Vec<LoadedSkill>,
}

impl Registry {
    /// Empty registry — no skills, tool-calling is a no-op.
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                host: SkillHost::new().expect("building empty skill host"),
                skills: Vec::new(),
            }),
        }
    }

    /// Load every skill in the given directory. A malformed skill is logged
    /// and skipped — one bad skill must not take the whole daemon down.
    pub fn load_from(dir: &Path) -> Self {
        let host = match SkillHost::new() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("bunzod: skill host init failed: {e:#}");
                return Self::empty();
            }
        };
        let mut skills = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("bunzod: skills dir {} unreadable: {e}", dir.display());
                }
                return Self {
                    inner: Arc::new(RegistryInner { host, skills }),
                };
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match load_one(&host, &path) {
                Ok(s) => {
                    eprintln!(
                        "bunzod: loaded skill {} from {}",
                        s.manifest.name,
                        path.display()
                    );
                    skills.push(s);
                }
                Err(e) => {
                    eprintln!("bunzod: skipping {}: {e:#}", path.display());
                }
            }
        }
        Self {
            inner: Arc::new(RegistryInner { host, skills }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.skills.is_empty()
    }

    pub fn tool_descriptors(&self) -> Vec<ToolDescriptor> {
        self.inner
            .skills
            .iter()
            .map(|s| ToolDescriptor {
                name: s.manifest.name.clone(),
                description: s.manifest.description.clone(),
                parameters: s.manifest.parameters.clone(),
            })
            .collect()
    }

    /// Invoke a skill by name with JSON args. Blocking — call from
    /// `spawn_blocking` to keep the tokio reactor responsive.
    pub fn invoke_sync(&self, name: &str, args_json: &str) -> Result<String> {
        let skill = self.find_skill(name)?;
        self.inner.host.invoke(&skill.skill, args_json)
    }

    pub fn capability_denial_for_invocation(&self, name: &str, args_json: &str) -> Option<String> {
        let skill = self.find_skill(name).ok()?;
        if skill.manifest.name == READ_LOCAL_FILE_SKILL {
            let path = read_local_file_path(args_json)?;
            if !skill.manifest.capabilities.allows_read(&path) {
                return Some(format!(
                    "skill manifest denies {READ_LOCAL_FILE_SKILL} fs_read for {path}"
                ));
            }
        }
        None
    }

    fn find_skill(&self, name: &str) -> Result<&LoadedSkill> {
        self.inner
            .skills
            .iter()
            .find(|s| s.manifest.name == name)
            .ok_or_else(|| anyhow!("unknown skill: {name}"))
    }
}

fn load_one(host: &SkillHost, dir: &Path) -> Result<LoadedSkill> {
    let (manifest, wasm_path) = manifest::load_dir(dir)?;
    let skill = host.build_skill(manifest.clone(), &wasm_path)?;
    Ok(LoadedSkill {
        manifest,
        path: dir.to_path_buf(),
        skill,
    })
}

pub fn default_dir() -> PathBuf {
    std::env::var_os("BUNZO_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SKILLS_DIR))
}
