//! bunzo-provisiond — canonical provisioning owner for setup state.
//!
//! Frontends stay narrow: shell and headless HTTP setup both call this service
//! so canonical provider config + secret live under `/var/lib/bunzo/`, the
//! restart-safe state machine advances in one place, and `/etc/bunzo/`
//! remains rendered runtime output rather than source of truth.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use bunzo_proto::async_io::{read_frame_async, write_frame_async};
use bunzo_proto::{
    Envelope, ProvisionClientFrame, ProvisionClientMessage, ProvisionServerMessage,
    ProvisioningSetupInput, ProvisioningStatus, PROTOCOL_VERSION,
};
use listenfd::ListenFd;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncWrite, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::backend::openai;
use crate::config::{BackendConfig, BunzodConfig, OpenAiConfig, RECOMMENDED_OPENAI_MODEL};

pub const SOCKET_PATH: &str = "/run/bunzo-provisiond.sock";
pub const DEFAULT_CONFIG_DIR: &str = "/var/lib/bunzo/config";
pub const DEFAULT_SECRETS_DIR: &str = "/var/lib/bunzo/secrets";
pub const DEFAULT_PROVISIONING_DIR: &str = "/var/lib/bunzo/provisioning";
pub const DEFAULT_RUNTIME_ROOT_DIR: &str = "/etc";
pub const DEFAULT_RUNTIME_CONFIG_DIR: &str = "/etc/bunzo";
pub const DEFAULT_RUNTIME_CONFIG_PATH: &str = "/etc/bunzo/bunzod.toml";
pub const DEFAULT_RUNTIME_HOSTNAME_PATH: &str = "/etc/hostname";
pub const DEFAULT_RUNTIME_NETWORK_INTERFACES_PATH: &str = "/etc/network/interfaces";

const DEVICE_CONFIG_NAME: &str = "device.toml";
const NETWORK_CONFIG_NAME: &str = "network.toml";
const PROVIDER_CONFIG_NAME: &str = "provider.toml";
const STATE_FILE_NAME: &str = "state.toml";
const OPENAI_SECRET_NAME: &str = "openai.key";
const CONNECTIVITY_KIND_EXISTING_NETWORK: &str = "existing_network";
const EXISTING_NETWORK_INTERFACE: &str = "eth0";
const PROVIDER_KIND_OPENAI: &str = "openai";
const FRONTEND_SOURCE_SHARED: &str = "provisioning_frontend";
const NETWORK_SERVICE_NAME: &str = "network.service";
const MAX_DEVICE_NAME_LEN: usize = 63;

#[derive(Debug, Clone)]
pub struct ProvisioningPaths {
    pub config_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub provisioning_dir: PathBuf,
    pub runtime_root_dir: PathBuf,
    pub runtime_config_dir: PathBuf,
    pub runtime_config_path: PathBuf,
    pub runtime_hostname_path: PathBuf,
    pub runtime_network_interfaces_path: PathBuf,
}

impl Default for ProvisioningPaths {
    fn default() -> Self {
        Self {
            config_dir: PathBuf::from(DEFAULT_CONFIG_DIR),
            secrets_dir: PathBuf::from(DEFAULT_SECRETS_DIR),
            provisioning_dir: PathBuf::from(DEFAULT_PROVISIONING_DIR),
            runtime_root_dir: PathBuf::from(DEFAULT_RUNTIME_ROOT_DIR),
            runtime_config_dir: PathBuf::from(DEFAULT_RUNTIME_CONFIG_DIR),
            runtime_config_path: PathBuf::from(DEFAULT_RUNTIME_CONFIG_PATH),
            runtime_hostname_path: PathBuf::from(DEFAULT_RUNTIME_HOSTNAME_PATH),
            runtime_network_interfaces_path: PathBuf::from(DEFAULT_RUNTIME_NETWORK_INTERFACES_PATH),
        }
    }
}

impl ProvisioningPaths {
    fn device_config_path(&self) -> PathBuf {
        self.config_dir.join(DEVICE_CONFIG_NAME)
    }

    fn network_config_path(&self) -> PathBuf {
        self.config_dir.join(NETWORK_CONFIG_NAME)
    }

    fn provider_config_path(&self) -> PathBuf {
        self.config_dir.join(PROVIDER_CONFIG_NAME)
    }

    fn state_path(&self) -> PathBuf {
        self.provisioning_dir.join(STATE_FILE_NAME)
    }

    fn openai_secret_path(&self) -> PathBuf {
        self.secrets_dir.join(OPENAI_SECRET_NAME)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProvisioningPhase {
    Unprovisioned,
    Naming,
    Connectivity,
    Provider,
    Validating,
    Ready,
    FailedRecoverable,
}

impl ProvisioningPhase {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Unprovisioned => "unprovisioned",
            Self::Naming => "naming",
            Self::Connectivity => "connectivity",
            Self::Provider => "provider",
            Self::Validating => "validating",
            Self::Ready => "ready",
            Self::FailedRecoverable => "failed_recoverable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProvisioningStateRecord {
    phase: ProvisioningPhase,
    device_name: Option<String>,
    connectivity_kind: Option<String>,
    provider_kind: Option<String>,
    model: Option<String>,
    rendered_config_path: Option<String>,
    secret_path: Option<String>,
    last_error: Option<String>,
    updated_at_ms: u64,
}

impl Default for ProvisioningStateRecord {
    fn default() -> Self {
        Self {
            phase: ProvisioningPhase::Unprovisioned,
            device_name: None,
            connectivity_kind: None,
            provider_kind: None,
            model: None,
            rendered_config_path: None,
            secret_path: None,
            last_error: None,
            updated_at_ms: now_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceConfig {
    device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkConfig {
    kind: String,
    source: String,
    #[serde(default = "default_existing_network_interface")]
    interface_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderConfig {
    backend: CanonicalBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CanonicalBackendConfig {
    Openai {
        model: String,
        api_key_secret: String,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        system_prompt: Option<String>,
    },
}

#[derive(Debug, Serialize)]
struct RenderedRuntimeConfig {
    backend: RenderedOpenAiConfig,
}

#[derive(Debug, Serialize)]
struct RenderedOpenAiConfig {
    kind: String,
    model: String,
    api_key_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
}

#[async_trait]
trait ProviderValidator: Send + Sync {
    async fn validate(
        &self,
        provider_cfg: &ProviderConfig,
        paths: &ProvisioningPaths,
    ) -> Result<()>;
}

trait RuntimeActivator: Send + Sync {
    fn set_live_hostname(&self, device_name: &str) -> Result<()>;
    fn restart_network_if_active(&self) -> Result<()>;
}

#[derive(Default)]
struct LiveProviderValidator;

#[derive(Default)]
struct LiveRuntimeActivator;

#[async_trait]
impl ProviderValidator for LiveProviderValidator {
    async fn validate(
        &self,
        provider_cfg: &ProviderConfig,
        paths: &ProvisioningPaths,
    ) -> Result<()> {
        match provider_cfg.openai_runtime_config(paths)? {
            Some(cfg) => openai::validate_access(&cfg).await,
            None => bail!("unsupported provider configuration"),
        }
    }
}

impl RuntimeActivator for LiveRuntimeActivator {
    fn set_live_hostname(&self, device_name: &str) -> Result<()> {
        let status = Command::new("hostname")
            .arg(device_name)
            .status()
            .with_context(|| format!("setting live hostname to '{device_name}'"))?;
        if !status.success() {
            bail!("hostname {device_name} exited with {status}");
        }
        Ok(())
    }

    fn restart_network_if_active(&self) -> Result<()> {
        let status = Command::new("systemctl")
            .args(["is-active", "--quiet", NETWORK_SERVICE_NAME])
            .status()
            .context("checking network.service state")?;
        if !status.success() {
            return Ok(());
        }

        let status = Command::new("systemctl")
            .args(["restart", NETWORK_SERVICE_NAME])
            .status()
            .context("restarting network.service")?;
        if !status.success() {
            bail!("systemctl restart {NETWORK_SERVICE_NAME} exited with {status}");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProvisioningEngine {
    paths: ProvisioningPaths,
    validator: Arc<dyn ProviderValidator>,
    activator: Arc<dyn RuntimeActivator>,
}

impl Default for ProvisioningEngine {
    fn default() -> Self {
        Self::with_components(
            ProvisioningPaths::default(),
            Arc::new(LiveProviderValidator),
            Arc::new(LiveRuntimeActivator),
        )
    }
}

impl ProvisioningEngine {
    pub fn new(paths: ProvisioningPaths) -> Self {
        Self::with_components(
            paths,
            Arc::new(LiveProviderValidator),
            Arc::new(LiveRuntimeActivator),
        )
    }

    fn with_components(
        paths: ProvisioningPaths,
        validator: Arc<dyn ProviderValidator>,
        activator: Arc<dyn RuntimeActivator>,
    ) -> Self {
        Self {
            paths,
            validator,
            activator,
        }
    }

    pub fn status(&self) -> Result<ProvisioningStatus> {
        self.reconcile_runtime_state()?;
        let state = self.load_state()?.unwrap_or_default();
        let mut effective_phase = state.phase.clone();
        let mut ready = matches!(effective_phase, ProvisioningPhase::Ready);
        let mut detail = state.last_error.clone();

        if ready {
            if let Err(e) = self.validate_rendered_runtime() {
                ready = false;
                effective_phase = ProvisioningPhase::FailedRecoverable;
                detail = Some(format!("{e:#}"));
            }
        }
        if detail.is_none() {
            detail = Some(default_phase_detail(&effective_phase).to_string());
        }

        Ok(self.status_from_state(&state, &effective_phase, ready, detail))
    }

    pub fn apply_local_setup(
        &self,
        requested_device_name: Option<&str>,
        api_key: &str,
    ) -> Result<ProvisioningStatus> {
        futures::executor::block_on(self.apply_local_setup_async(requested_device_name, api_key))
    }

    pub async fn apply_local_setup_async(
        &self,
        requested_device_name: Option<&str>,
        api_key: &str,
    ) -> Result<ProvisioningStatus> {
        self.apply_setup_async(&ProvisioningSetupInput {
            device_name: requested_device_name.map(str::to_string),
            connectivity_kind: None,
            provider_kind: None,
            api_key: api_key.to_string(),
        })
        .await
    }

    pub fn apply_setup(&self, setup: &ProvisioningSetupInput) -> Result<ProvisioningStatus> {
        futures::executor::block_on(self.apply_setup_async(setup))
    }

    pub async fn apply_setup_async(
        &self,
        setup: &ProvisioningSetupInput,
    ) -> Result<ProvisioningStatus> {
        let api_key = setup.api_key.trim();
        if api_key.is_empty() {
            bail!("api key cannot be empty");
        }

        self.ensure_dirs()?;

        let mut state = self.load_state()?.unwrap_or_default();
        let device_name = match self.resolve_device_name(setup.device_name.as_deref()) {
            Ok(device_name) => device_name,
            Err(err) => {
                self.write_failed_state(&state, &format!("{err:#}"))?;
                return Err(err);
            }
        };
        let device_cfg = DeviceConfig { device_name };

        state.device_name = Some(device_cfg.device_name.clone());
        state.last_error = None;
        self.write_state_checkpoint(&mut state, ProvisioningPhase::Naming)?;
        self.write_toml_atomic(&self.paths.device_config_path(), &device_cfg, 0o600)?;

        let connectivity_kind = setup
            .connectivity_kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .unwrap_or(CONNECTIVITY_KIND_EXISTING_NETWORK);
        if connectivity_kind != CONNECTIVITY_KIND_EXISTING_NETWORK {
            let err = anyhow!(
                "unsupported connectivity kind '{connectivity_kind}'; this slice only supports '{CONNECTIVITY_KIND_EXISTING_NETWORK}'"
            );
            self.write_failed_state(&state, &format!("{err:#}"))?;
            return Err(err);
        }

        let network_cfg = NetworkConfig {
            kind: connectivity_kind.into(),
            source: FRONTEND_SOURCE_SHARED.into(),
            interface_name: default_existing_network_interface(),
        };
        state.connectivity_kind = Some(connectivity_kind.into());
        self.write_state_checkpoint(&mut state, ProvisioningPhase::Connectivity)?;
        self.write_toml_atomic(&self.paths.network_config_path(), &network_cfg, 0o600)?;

        let provider_kind = setup
            .provider_kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .unwrap_or(PROVIDER_KIND_OPENAI);
        if provider_kind != PROVIDER_KIND_OPENAI {
            let err = anyhow!(
                "unsupported provider kind '{provider_kind}'; this slice only supports '{PROVIDER_KIND_OPENAI}'"
            );
            self.write_failed_state(&state, &format!("{err:#}"))?;
            return Err(err);
        }

        let provider_cfg = ProviderConfig {
            backend: CanonicalBackendConfig::Openai {
                model: RECOMMENDED_OPENAI_MODEL.into(),
                api_key_secret: OPENAI_SECRET_NAME.into(),
                base_url: None,
                system_prompt: None,
            },
        };
        state.provider_kind = Some(provider_kind.into());
        state.model = Some(RECOMMENDED_OPENAI_MODEL.into());
        state.secret_path = Some(self.paths.openai_secret_path().display().to_string());
        state.rendered_config_path = Some(self.paths.runtime_config_path.display().to_string());
        self.write_state_checkpoint(&mut state, ProvisioningPhase::Provider)?;
        self.write_toml_atomic(&self.paths.provider_config_path(), &provider_cfg, 0o600)?;
        self.write_string_atomic(
            &self.paths.openai_secret_path(),
            &format!("{api_key}\n"),
            0o600,
        )?;

        state.last_error = None;
        self.write_state_checkpoint(&mut state, ProvisioningPhase::Validating)?;

        if let Err(err) = self
            .apply_runtime_hostname(&device_cfg)
            .and_then(|_| self.apply_runtime_network(&network_cfg))
            .and_then(|_| self.render_runtime_config(&provider_cfg))
            .and_then(|_| self.validate_rendered_runtime())
        {
            self.write_failed_state(&state, &format!("{err:#}"))?;
            return Err(err);
        }
        if let Err(err) = self.validator.validate(&provider_cfg, &self.paths).await {
            self.write_failed_state(&state, &format!("{err:#}"))?;
            return Err(err);
        }

        state.last_error = None;
        self.write_state_checkpoint(&mut state, ProvisioningPhase::Ready)?;
        self.status()
    }

    pub fn reconcile_runtime_state(&self) -> Result<()> {
        self.ensure_dirs()?;

        let mut state = match self.load_state()? {
            Some(state) => state,
            None => return Ok(()),
        };
        let original = state.clone();

        let device_cfg =
            self.read_optional_toml::<DeviceConfig>(&self.paths.device_config_path())?;
        if let Some(device) = &device_cfg {
            state.device_name = Some(device.device_name.clone());
        }

        let network_cfg =
            self.read_optional_toml::<NetworkConfig>(&self.paths.network_config_path())?;
        if let Some(network) = &network_cfg {
            state.connectivity_kind = Some(network.kind.clone());
        }

        let provider_cfg =
            self.read_optional_toml::<ProviderConfig>(&self.paths.provider_config_path())?;
        if let Some(provider_cfg) = &provider_cfg {
            self.apply_provider_metadata(&mut state, provider_cfg);
        }

        match self.reconcile_runtime_outputs(
            device_cfg.as_ref(),
            network_cfg.as_ref(),
            provider_cfg.as_ref(),
        ) {
            Ok(()) => {
                if matches!(state.phase, ProvisioningPhase::Ready) {
                    state.last_error = None;
                }
                if state != original {
                    self.persist_state(&mut state)?;
                }
            }
            Err(err) => {
                self.write_failed_state(&state, &format!("{err:#}"))?;
            }
        }

        Ok(())
    }

    fn ensure_dirs(&self) -> Result<()> {
        ensure_dir_mode(&self.paths.config_dir, 0o700)?;
        ensure_dir_mode(&self.paths.secrets_dir, 0o700)?;
        ensure_dir_mode(&self.paths.provisioning_dir, 0o700)?;
        ensure_dir_mode(&self.paths.runtime_config_dir, 0o755)?;
        Ok(())
    }

    fn load_state(&self) -> Result<Option<ProvisioningStateRecord>> {
        self.read_optional_toml::<ProvisioningStateRecord>(&self.paths.state_path())
    }

    fn status_from_state(
        &self,
        state: &ProvisioningStateRecord,
        phase: &ProvisioningPhase,
        ready: bool,
        detail: Option<String>,
    ) -> ProvisioningStatus {
        ProvisioningStatus {
            phase: phase.as_str().into(),
            ready,
            device_name: state.device_name.clone(),
            connectivity_kind: state.connectivity_kind.clone(),
            provider_kind: state.provider_kind.clone(),
            model: state.model.clone(),
            rendered_config_path: state.rendered_config_path.clone(),
            secret_path: state.secret_path.clone(),
            detail,
            updated_at_ms: state.updated_at_ms,
        }
    }

    fn write_state_checkpoint(
        &self,
        state: &mut ProvisioningStateRecord,
        phase: ProvisioningPhase,
    ) -> Result<()> {
        state.phase = phase;
        self.persist_state(state)
    }

    fn write_failed_state(&self, prior: &ProvisioningStateRecord, detail: &str) -> Result<()> {
        let mut failed = prior.clone();
        failed.phase = ProvisioningPhase::FailedRecoverable;
        failed.last_error = Some(detail.to_string());
        self.persist_state(&mut failed)
    }

    fn persist_state(&self, state: &mut ProvisioningStateRecord) -> Result<()> {
        state.updated_at_ms = now_ms();
        self.write_toml_atomic(&self.paths.state_path(), state, 0o600)
    }

    fn apply_provider_metadata(
        &self,
        state: &mut ProvisioningStateRecord,
        provider_cfg: &ProviderConfig,
    ) {
        match &provider_cfg.backend {
            CanonicalBackendConfig::Openai { model, .. } => {
                state.provider_kind = Some(PROVIDER_KIND_OPENAI.into());
                state.model = Some(model.clone());
                state.secret_path = Some(self.paths.openai_secret_path().display().to_string());
                state.rendered_config_path =
                    Some(self.paths.runtime_config_path.display().to_string());
            }
        }
    }

    fn resolve_device_name(&self, requested_device_name: Option<&str>) -> Result<String> {
        if let Some(requested) = requested_device_name {
            return normalize_device_name(requested);
        }

        self.read_toml::<DeviceConfig>(&self.paths.device_config_path())
            .ok()
            .map(|cfg| cfg.device_name)
            .map(Ok)
            .unwrap_or_else(|| Ok(self.current_device_name()))
    }

    fn current_device_name(&self) -> String {
        fs::read_to_string(&self.paths.runtime_hostname_path)
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|name| !name.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "bunzo".into())
    }

    fn reconcile_runtime_outputs(
        &self,
        device_cfg: Option<&DeviceConfig>,
        network_cfg: Option<&NetworkConfig>,
        provider_cfg: Option<&ProviderConfig>,
    ) -> Result<()> {
        if let Some(device_cfg) = device_cfg {
            self.apply_runtime_hostname(device_cfg)?;
        }
        if let Some(network_cfg) = network_cfg {
            self.apply_runtime_network(network_cfg)?;
        }
        if let Some(provider_cfg) = provider_cfg {
            self.render_runtime_config(provider_cfg)?;
            self.validate_rendered_runtime()?;
        }
        Ok(())
    }

    fn apply_runtime_hostname(&self, device_cfg: &DeviceConfig) -> Result<()> {
        let device_name = normalize_device_name(&device_cfg.device_name)?;
        let hostname_body = format!("{device_name}\n");
        self.write_string_if_changed(&self.paths.runtime_hostname_path, &hostname_body, 0o644)?;
        self.activator.set_live_hostname(&device_name)
    }

    fn apply_runtime_network(&self, network_cfg: &NetworkConfig) -> Result<()> {
        if network_cfg.kind != CONNECTIVITY_KIND_EXISTING_NETWORK {
            bail!(
                "unsupported connectivity kind '{}'; this slice only supports '{CONNECTIVITY_KIND_EXISTING_NETWORK}'",
                network_cfg.kind
            );
        }

        let interface_name = normalize_network_interface_name(&network_cfg.interface_name)?;
        let body = render_existing_network_interfaces(&interface_name);
        let changed = self.write_string_if_changed(
            &self.paths.runtime_network_interfaces_path,
            &body,
            0o644,
        )?;
        if changed {
            self.activator.restart_network_if_active()?;
        }
        Ok(())
    }

    fn render_runtime_config(&self, provider_cfg: &ProviderConfig) -> Result<()> {
        let rendered = match &provider_cfg.backend {
            CanonicalBackendConfig::Openai {
                model,
                api_key_secret,
                base_url,
                system_prompt,
            } => RenderedRuntimeConfig {
                backend: RenderedOpenAiConfig {
                    kind: PROVIDER_KIND_OPENAI.into(),
                    model: model.clone(),
                    api_key_path: self
                        .paths
                        .secrets_dir
                        .join(api_key_secret)
                        .display()
                        .to_string(),
                    base_url: base_url.clone(),
                    system_prompt: system_prompt.clone(),
                },
            },
        };
        let mut body = toml::to_string(&rendered).context("serializing rendered runtime config")?;
        body.insert_str(
            0,
            "# Rendered by bunzo-provisiond from /var/lib/bunzo/config/provider.toml.\n",
        );
        self.write_string_atomic(&self.paths.runtime_config_path, &body, 0o644)
    }

    fn validate_rendered_runtime(&self) -> Result<()> {
        let raw = fs::read_to_string(&self.paths.runtime_config_path)
            .with_context(|| format!("reading {}", self.paths.runtime_config_path.display()))?;
        let cfg: BunzodConfig = toml::from_str(&raw)
            .with_context(|| format!("parsing {}", self.paths.runtime_config_path.display()))?;
        match cfg.backend {
            BackendConfig::Openai(oai) => {
                oai.validate().with_context(|| {
                    format!("validating {}", self.paths.runtime_config_path.display())
                })?;
                let key = fs::read_to_string(&oai.api_key_path).with_context(|| {
                    format!("reading api key from {}", oai.api_key_path.display())
                })?;
                if key.trim().is_empty() {
                    bail!("api key file {} is empty", oai.api_key_path.display());
                }
            }
        }
        Ok(())
    }

    fn read_optional_toml<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        match self.read_toml::<T>(path) {
            Ok(value) => Ok(Some(value)),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn read_toml<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let raw =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    fn write_toml_atomic<T: Serialize>(&self, path: &Path, value: &T, mode: u32) -> Result<()> {
        let mut body =
            toml::to_string(value).with_context(|| format!("serializing {}", path.display()))?;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        self.write_string_atomic(path, &body, mode)
    }

    fn write_string_if_changed(&self, path: &Path, contents: &str, mode: u32) -> Result<bool> {
        if let Ok(existing) = fs::read_to_string(path) {
            if existing == contents {
                fs::set_permissions(path, fs::Permissions::from_mode(mode))
                    .with_context(|| format!("chmod {:o} {}", mode, path.display()))?;
                return Ok(false);
            }
        }
        self.write_string_atomic(path, contents, mode)?;
        Ok(true)
    }

    fn write_string_atomic(&self, path: &Path, contents: &str, mode: u32) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("{} has no parent directory", path.display()))?;
        ensure_dir_mode(
            parent,
            if path.starts_with(&self.paths.runtime_root_dir) {
                0o755
            } else {
                0o700
            },
        )?;
        let tmp_path = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("bunzo"),
            now_ms()
        ));
        fs::write(&tmp_path, contents)
            .with_context(|| format!("writing {}", tmp_path.display()))?;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {:o} {}", mode, tmp_path.display()))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
        Ok(())
    }
}

impl ProviderConfig {
    fn openai_runtime_config(&self, paths: &ProvisioningPaths) -> Result<Option<OpenAiConfig>> {
        match &self.backend {
            CanonicalBackendConfig::Openai {
                model,
                api_key_secret,
                base_url,
                system_prompt,
            } => Ok(Some(OpenAiConfig {
                model: model.clone(),
                api_key_path: paths.secrets_dir.join(api_key_secret),
                base_url: base_url.clone(),
                system_prompt: system_prompt.clone(),
            })),
        }
    }
}

pub fn reconcile_runtime_state() -> Result<()> {
    ProvisioningEngine::default().reconcile_runtime_state()
}

pub async fn run_server() -> Result<()> {
    let listener = acquire_listener()?;
    let engine = ProvisioningEngine::default();
    if let Err(err) = engine.reconcile_runtime_state() {
        eprintln!("bunzo-provisiond: startup reconciliation failed: {err:#}");
    }
    eprintln!("bunzo-provisiond: accepting connections");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, engine).await {
                eprintln!("bunzo-provisiond: connection ended: {err:#}");
            }
        });
    }
}

fn acquire_listener() -> Result<UnixListener> {
    let mut listenfd = ListenFd::from_env();
    if let Some(std_listener) = listenfd.take_unix_listener(0)? {
        std_listener.set_nonblocking(true)?;
        eprintln!("bunzo-provisiond: using socket-activated listener from systemd");
        return UnixListener::from_std(std_listener).context("wrapping inherited listener");
    }

    let path = Path::new(SOCKET_PATH);
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing stale {SOCKET_PATH}"))?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("binding {SOCKET_PATH}"))?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o660));
    eprintln!("bunzo-provisiond: bound {SOCKET_PATH} directly");
    Ok(listener)
}

async fn handle_connection(mut stream: UnixStream, engine: ProvisioningEngine) -> Result<()> {
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    loop {
        let frame: ProvisionClientFrame = match read_frame_async(&mut reader).await {
            Ok(frame) => frame,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err.into()),
        };

        if frame.v != PROTOCOL_VERSION {
            let err = Envelope::new(ProvisionServerMessage::Error {
                id: String::new(),
                code: "unsupported_version".into(),
                text: format!(
                    "client speaks v{}, bunzo-provisiond speaks v{PROTOCOL_VERSION}",
                    frame.v
                ),
            });
            write_frame_async(&mut write_half, &err).await?;
            continue;
        }

        match frame.msg {
            ProvisionClientMessage::GetProvisioningStatus { id } => {
                handle_get_status(&mut write_half, &id, &engine).await?;
            }
            ProvisionClientMessage::ApplySetup { id, setup } => {
                handle_apply_setup(&mut write_half, &id, &setup, &engine).await?;
            }
        }
    }
}

async fn handle_get_status<W>(w: &mut W, id: &str, engine: &ProvisioningEngine) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match engine.status() {
        Ok(status) => {
            let frame = Envelope::new(ProvisionServerMessage::ProvisioningStatus {
                id: id.into(),
                status,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(err) => {
            let frame = Envelope::new(ProvisionServerMessage::Error {
                id: id.into(),
                code: "provisioning_status_failed".into(),
                text: format!("{err:#}"),
            });
            write_frame_async(w, &frame).await?;
        }
    }
    Ok(())
}

async fn handle_apply_setup<W>(
    w: &mut W,
    id: &str,
    setup: &ProvisioningSetupInput,
    engine: &ProvisioningEngine,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match engine.apply_setup_async(setup).await {
        Ok(status) => {
            let frame = Envelope::new(ProvisionServerMessage::ProvisioningResult {
                id: id.into(),
                status,
            });
            write_frame_async(w, &frame).await?;
        }
        Err(err) => {
            let code = if setup.api_key.trim().is_empty() {
                "invalid_request"
            } else {
                "provisioning_apply_failed"
            };
            let frame = Envelope::new(ProvisionServerMessage::Error {
                id: id.into(),
                code: code.into(),
                text: format!("{err:#}"),
            });
            write_frame_async(w, &frame).await?;
        }
    }
    Ok(())
}

fn default_phase_detail(phase: &ProvisioningPhase) -> &'static str {
    match phase {
        ProvisioningPhase::Unprovisioned => "setup has not completed yet",
        ProvisioningPhase::Naming => "device identity is being persisted",
        ProvisioningPhase::Connectivity => "runtime connectivity config is being persisted",
        ProvisioningPhase::Provider => "provider config is being persisted",
        ProvisioningPhase::Validating => "rendered runtime config is being validated",
        ProvisioningPhase::Ready => "runtime config is ready",
        ProvisioningPhase::FailedRecoverable => "setup failed and can be retried",
    }
}

fn default_existing_network_interface() -> String {
    EXISTING_NETWORK_INTERFACE.into()
}

fn normalize_device_name(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("device name cannot be empty");
    }
    if trimmed.len() > MAX_DEVICE_NAME_LEN {
        bail!("device name '{trimmed}' exceeds {MAX_DEVICE_NAME_LEN} characters");
    }
    let bytes = trimmed.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        bail!("device name '{trimmed}' must start and end with an ASCII letter or digit");
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        bail!("device name '{trimmed}' must contain only ASCII letters, digits, or hyphens");
    }
    Ok(trimmed.to_string())
}

fn normalize_network_interface_name(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("network interface name cannot be empty");
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        bail!("network interface name '{trimmed}' contains unsupported characters");
    }
    Ok(trimmed.to_string())
}

fn render_existing_network_interfaces(interface_name: &str) -> String {
    format!(
        concat!(
            "# interface file auto-generated by buildroot\n",
            "\n",
            "auto lo\n",
            "iface lo inet loopback\n",
            "\n",
            "auto {interface_name}\n",
            "iface {interface_name} inet dhcp\n",
            "  pre-up /etc/network/nfs_check\n",
            "  wait-delay 15\n",
            "  hostname $(hostname)\n"
        ),
        interface_name = interface_name
    )
}

fn ensure_dir_mode(path: &Path, mode: u32) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("chmod {:o} {}", mode, path.display()))
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::bail;
    use tempfile::TempDir;

    #[derive(Clone)]
    struct StubValidator {
        result: std::result::Result<(), String>,
    }

    impl StubValidator {
        fn success() -> Arc<Self> {
            Arc::new(Self { result: Ok(()) })
        }

        fn failure(message: &str) -> Arc<Self> {
            Arc::new(Self {
                result: Err(message.to_string()),
            })
        }
    }

    #[async_trait]
    impl ProviderValidator for StubValidator {
        async fn validate(
            &self,
            _provider_cfg: &ProviderConfig,
            _paths: &ProvisioningPaths,
        ) -> Result<()> {
            match &self.result {
                Ok(()) => Ok(()),
                Err(message) => bail!("{message}"),
            }
        }
    }

    struct StubRuntimeActivator {
        hostname_result: std::result::Result<(), String>,
        network_result: std::result::Result<(), String>,
        hostname_calls: Mutex<Vec<String>>,
        network_restart_calls: AtomicUsize,
    }

    impl StubRuntimeActivator {
        fn success() -> Arc<Self> {
            Arc::new(Self {
                hostname_result: Ok(()),
                network_result: Ok(()),
                hostname_calls: Mutex::new(Vec::new()),
                network_restart_calls: AtomicUsize::new(0),
            })
        }

        fn hostname_failure(message: &str) -> Arc<Self> {
            Arc::new(Self {
                hostname_result: Err(message.to_string()),
                network_result: Ok(()),
                hostname_calls: Mutex::new(Vec::new()),
                network_restart_calls: AtomicUsize::new(0),
            })
        }
    }

    impl RuntimeActivator for StubRuntimeActivator {
        fn set_live_hostname(&self, device_name: &str) -> Result<()> {
            self.hostname_calls
                .lock()
                .unwrap()
                .push(device_name.to_string());
            match &self.hostname_result {
                Ok(()) => Ok(()),
                Err(message) => bail!("{message}"),
            }
        }

        fn restart_network_if_active(&self) -> Result<()> {
            self.network_restart_calls.fetch_add(1, Ordering::Relaxed);
            match &self.network_result {
                Ok(()) => Ok(()),
                Err(message) => bail!("{message}"),
            }
        }
    }

    fn test_paths(tmp: &TempDir) -> ProvisioningPaths {
        let root = tmp.path();
        let runtime_root = root.join("etc");
        fs::create_dir_all(runtime_root.join("network")).unwrap();
        fs::write(runtime_root.join("hostname"), "bunzo\n").unwrap();
        ProvisioningPaths {
            config_dir: root.join("var/lib/bunzo/config"),
            secrets_dir: root.join("var/lib/bunzo/secrets"),
            provisioning_dir: root.join("var/lib/bunzo/provisioning"),
            runtime_root_dir: runtime_root.clone(),
            runtime_config_dir: root.join("etc/bunzo"),
            runtime_config_path: root.join("etc/bunzo/bunzod.toml"),
            runtime_hostname_path: runtime_root.join("hostname"),
            runtime_network_interfaces_path: runtime_root.join("network/interfaces"),
        }
    }

    fn test_engine(
        tmp: &TempDir,
        validator: Arc<dyn ProviderValidator>,
        activator: Arc<dyn RuntimeActivator>,
    ) -> ProvisioningEngine {
        ProvisioningEngine::with_components(test_paths(tmp), validator, activator)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_setup_writes_canonical_state_and_runtime_outputs() {
        let tmp = TempDir::new().unwrap();
        let activator = StubRuntimeActivator::success();
        let engine = test_engine(&tmp, StubValidator::success(), activator.clone());

        let status = engine
            .apply_local_setup_async(Some("bunzo-qemu"), "sk-test")
            .await
            .unwrap();
        assert!(status.ready);
        assert_eq!(status.phase, "ready");
        assert_eq!(status.device_name.as_deref(), Some("bunzo-qemu"));
        assert_eq!(status.provider_kind.as_deref(), Some("openai"));
        assert_eq!(status.model.as_deref(), Some("gpt-5.4-mini"));

        let device: DeviceConfig = engine
            .read_toml(&engine.paths.device_config_path())
            .expect("device config");
        assert_eq!(device.device_name, "bunzo-qemu");

        let network: NetworkConfig = engine
            .read_toml(&engine.paths.network_config_path())
            .expect("network config");
        assert_eq!(network.kind, "existing_network");
        assert_eq!(network.interface_name, "eth0");

        let provider: ProviderConfig = engine
            .read_toml(&engine.paths.provider_config_path())
            .expect("provider config");
        match provider.backend {
            CanonicalBackendConfig::Openai {
                model,
                api_key_secret,
                ..
            } => {
                assert_eq!(model, "gpt-5.4-mini");
                assert_eq!(api_key_secret, "openai.key");
            }
        }

        let runtime_cfg = fs::read_to_string(&engine.paths.runtime_config_path).unwrap();
        assert!(runtime_cfg.contains("kind = \"openai\""));
        assert!(runtime_cfg.contains(
            engine
                .paths
                .openai_secret_path()
                .display()
                .to_string()
                .as_str()
        ));

        let secret = fs::read_to_string(engine.paths.openai_secret_path()).unwrap();
        assert_eq!(secret.trim(), "sk-test");

        let hostname = fs::read_to_string(&engine.paths.runtime_hostname_path).unwrap();
        assert_eq!(hostname.trim(), "bunzo-qemu");

        let interfaces = fs::read_to_string(&engine.paths.runtime_network_interfaces_path).unwrap();
        assert!(interfaces.contains("auto eth0"));
        assert!(interfaces.contains("hostname $(hostname)"));

        let hostname_calls = activator.hostname_calls.lock().unwrap().clone();
        assert_eq!(
            hostname_calls,
            vec!["bunzo-qemu".to_string(), "bunzo-qemu".to_string()]
        );
        assert_eq!(activator.network_restart_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_device_name_stays_non_ready_with_detail() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::success(),
            StubRuntimeActivator::success(),
        );

        let err = engine
            .apply_local_setup_async(Some("bunzo qemu"), "sk-test")
            .await
            .expect_err("device name should fail");
        assert!(format!("{err:#}").contains("must contain only ASCII letters, digits, or hyphens"));

        let status = engine.status().unwrap();
        assert!(!status.ready);
        assert_eq!(status.phase, "failed_recoverable");
        assert!(status
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("must contain only ASCII letters, digits, or hyphens"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hostname_activation_failures_stay_recoverable() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::success(),
            StubRuntimeActivator::hostname_failure("sethostname failed"),
        );

        let err = engine
            .apply_local_setup_async(Some("bunzo-qemu"), "sk-test")
            .await
            .expect_err("hostname activation should fail");
        assert!(format!("{err:#}").contains("sethostname failed"));

        let status = engine.status().unwrap();
        assert!(!status.ready);
        assert_eq!(status.phase, "failed_recoverable");
        assert!(status
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("sethostname failed"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_live_validation_stays_non_ready_with_detail() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::failure("authentication failed"),
            StubRuntimeActivator::success(),
        );

        let err = engine
            .apply_local_setup_async(None, "sk-test")
            .await
            .expect_err("validation should fail");
        assert!(format!("{err:#}").contains("authentication failed"));

        let status = engine.status().unwrap();
        assert!(!status.ready);
        assert_eq!(status.phase, "failed_recoverable");
        assert!(status
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("authentication failed"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_recreates_runtime_config_for_ready_state() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::success(),
            StubRuntimeActivator::success(),
        );
        engine
            .apply_local_setup_async(Some("bunzo-qemu"), "sk-test")
            .await
            .unwrap();

        fs::write(&engine.paths.runtime_config_path, "broken = true\n").unwrap();
        fs::write(&engine.paths.runtime_hostname_path, "wrong\n").unwrap();
        engine.reconcile_runtime_state().unwrap();

        let status = engine.status().unwrap();
        assert!(status.ready);
        let runtime_cfg = fs::read_to_string(&engine.paths.runtime_config_path).unwrap();
        assert!(runtime_cfg.contains("kind = \"openai\""));
        assert!(!runtime_cfg.contains("broken = true"));
        let hostname = fs::read_to_string(&engine.paths.runtime_hostname_path).unwrap();
        assert_eq!(hostname.trim(), "bunzo-qemu");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_does_not_promote_failed_validation_to_ready() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::failure("authentication failed"),
            StubRuntimeActivator::success(),
        );
        let _ = engine.apply_local_setup_async(None, "sk-test").await;

        fs::remove_file(&engine.paths.runtime_config_path).unwrap();
        engine.reconcile_runtime_state().unwrap();

        let status = engine.status().unwrap();
        assert!(!status.ready);
        assert_eq!(status.phase, "failed_recoverable");
        assert!(engine.paths.runtime_config_path.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn broken_rendered_runtime_reports_failed_recoverable() {
        let tmp = TempDir::new().unwrap();
        let engine = test_engine(
            &tmp,
            StubValidator::success(),
            StubRuntimeActivator::success(),
        );
        engine
            .apply_local_setup_async(None, "sk-test")
            .await
            .unwrap();

        fs::remove_file(engine.paths.openai_secret_path()).unwrap();
        let status = engine.status().unwrap();
        assert!(!status.ready);
        assert_eq!(status.phase, "failed_recoverable");
        assert!(status
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("reading api key from"));
    }
}
