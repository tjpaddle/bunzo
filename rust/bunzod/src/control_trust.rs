//! Browser-control pairing and trusted-session state.
//!
//! This is HTTP transport trust state, not a second runtime path. Browser
//! actions still execute through bunzod's normal socket protocol after this
//! layer authenticates the caller.

use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::{rngs::OsRng, Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_CONTROL_DIR: &str = "/var/lib/bunzo/control";
pub const PAIRING_CODE_FILE_NAME: &str = "pairing-code";
pub const TRUST_STATE_FILE_NAME: &str = "trust.toml";
pub const SESSION_COOKIE_NAME: &str = "bunzo_control_session";
pub const SESSION_MAX_AGE_SECONDS: u64 = 365 * 24 * 60 * 60;

const TRUST_STATE_VERSION: u8 = 1;
const PAIRING_CODE_DIGITS: usize = 10;
const PAIRING_CODE_TTL_MS: u64 = 15 * 60 * 1000;
const PAIRING_LOCK_MS: u64 = 60 * 1000;
const PAIRING_MAX_FAILURES: u32 = 5;
const MAX_TRUSTED_SESSIONS: usize = 8;
const SESSION_TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone)]
pub struct BrowserTrustStore {
    control_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PairingChallengeSummary {
    pub code_path: PathBuf,
    pub expires_at_ms: u64,
    pub locked_until_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TrustedBrowserSession {
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
}

#[derive(Debug)]
pub enum PairingError {
    InvalidCode,
    ExpiredCode,
    Locked { until_ms: u64 },
    Store(anyhow::Error),
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCode => write!(f, "pairing code was not accepted"),
            Self::ExpiredCode => write!(f, "pairing code expired; a new code was generated"),
            Self::Locked { until_ms } => {
                write!(
                    f,
                    "too many failed pairing attempts; locked until {until_ms}"
                )
            }
            Self::Store(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for PairingError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserTrustState {
    #[serde(default = "default_trust_state_version")]
    version: u8,
    #[serde(default)]
    pairing: Option<PairingChallengeRecord>,
    #[serde(default)]
    sessions: Vec<TrustedSessionRecord>,
}

impl Default for BrowserTrustState {
    fn default() -> Self {
        Self {
            version: TRUST_STATE_VERSION,
            pairing: None,
            sessions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingChallengeRecord {
    code_hash: String,
    created_at_ms: u64,
    expires_at_ms: u64,
    #[serde(default)]
    failed_attempts: u32,
    #[serde(default)]
    locked_until_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustedSessionRecord {
    token_hash: String,
    created_at_ms: u64,
    last_seen_at_ms: u64,
}

impl Default for BrowserTrustStore {
    fn default() -> Self {
        let control_dir = std::env::var_os("BUNZO_CONTROL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_DIR));
        Self { control_dir }
    }
}

impl BrowserTrustStore {
    pub fn new<P: Into<PathBuf>>(control_dir: P) -> Self {
        Self {
            control_dir: control_dir.into(),
        }
    }

    pub fn pairing_code_path(&self) -> PathBuf {
        self.control_dir.join(PAIRING_CODE_FILE_NAME)
    }

    fn trust_state_path(&self) -> PathBuf {
        self.control_dir.join(TRUST_STATE_FILE_NAME)
    }

    pub fn ensure_pairing_challenge(&self) -> Result<PairingChallengeSummary> {
        self.ensure_dir()?;
        let now = now_ms();
        let mut state = self.load_state()?.unwrap_or_default();
        state.version = TRUST_STATE_VERSION;
        retain_fresh_sessions(&mut state, now);

        let mut needs_persist = false;
        if let Some(pairing) = state.pairing.as_mut() {
            if pairing.locked_until_ms.is_some_and(|until| until <= now) {
                pairing.locked_until_ms = None;
                pairing.failed_attempts = 0;
                needs_persist = true;
            }
        }

        let challenge_is_usable = state.pairing.as_ref().is_some_and(|pairing| {
            pairing.expires_at_ms > now && self.pairing_code_path().exists()
        });

        if !challenge_is_usable {
            let (code, pairing) = new_pairing_challenge(now);
            self.write_pairing_code(&code)?;
            state.pairing = Some(pairing);
            needs_persist = true;
        }

        if needs_persist {
            self.write_state(&state)?;
        }

        let pairing = state
            .pairing
            .as_ref()
            .context("pairing challenge missing after generation")?;
        Ok(PairingChallengeSummary {
            code_path: self.pairing_code_path(),
            expires_at_ms: pairing.expires_at_ms,
            locked_until_ms: pairing.locked_until_ms,
        })
    }

    pub fn pair_with_code(
        &self,
        submitted_code: &str,
    ) -> std::result::Result<String, PairingError> {
        self.ensure_dir().map_err(PairingError::Store)?;
        let now = now_ms();
        let mut state = self
            .load_state()
            .map_err(PairingError::Store)?
            .unwrap_or_default();
        state.version = TRUST_STATE_VERSION;
        retain_fresh_sessions(&mut state, now);

        let Some(mut pairing) = state.pairing.clone() else {
            let (code, next_pairing) = new_pairing_challenge(now);
            self.write_pairing_code(&code)
                .map_err(PairingError::Store)?;
            state.pairing = Some(next_pairing);
            self.write_state(&state).map_err(PairingError::Store)?;
            return Err(PairingError::ExpiredCode);
        };

        if let Some(until_ms) = pairing.locked_until_ms {
            if until_ms > now {
                return Err(PairingError::Locked { until_ms });
            }
            pairing.locked_until_ms = None;
            pairing.failed_attempts = 0;
        }

        if pairing.expires_at_ms <= now {
            let (code, next_pairing) = new_pairing_challenge(now);
            self.write_pairing_code(&code)
                .map_err(PairingError::Store)?;
            state.pairing = Some(next_pairing);
            self.write_state(&state).map_err(PairingError::Store)?;
            return Err(PairingError::ExpiredCode);
        }

        let normalized = normalize_pairing_code(submitted_code);
        if normalized.len() != PAIRING_CODE_DIGITS
            || !constant_time_eq(&hash_secret(&normalized), &pairing.code_hash)
        {
            pairing.failed_attempts = pairing.failed_attempts.saturating_add(1);
            if pairing.failed_attempts >= PAIRING_MAX_FAILURES {
                pairing.locked_until_ms = Some(now.saturating_add(PAIRING_LOCK_MS));
                pairing.failed_attempts = 0;
            }
            state.pairing = Some(pairing);
            self.write_state(&state).map_err(PairingError::Store)?;
            return Err(PairingError::InvalidCode);
        }

        let token = random_hex(SESSION_TOKEN_BYTES);
        let token_hash = hash_secret(&token);
        state.sessions.push(TrustedSessionRecord {
            token_hash,
            created_at_ms: now,
            last_seen_at_ms: now,
        });
        prune_oldest_sessions(&mut state);

        let (next_code, next_pairing) = new_pairing_challenge(now);
        self.write_pairing_code(&next_code)
            .map_err(PairingError::Store)?;
        state.pairing = Some(next_pairing);
        self.write_state(&state).map_err(PairingError::Store)?;

        Ok(token)
    }

    pub fn trusted_session_token(&self, token: &str) -> Result<Option<TrustedBrowserSession>> {
        let token = token.trim();
        if token.len() != SESSION_TOKEN_BYTES * 2 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }

        self.ensure_dir()?;
        let now = now_ms();
        let mut state = self.load_state()?.unwrap_or_default();
        state.version = TRUST_STATE_VERSION;
        retain_fresh_sessions(&mut state, now);
        let token_hash = hash_secret(token);

        let mut trusted = None;
        for session in &mut state.sessions {
            if constant_time_eq(&session.token_hash, &token_hash) {
                session.last_seen_at_ms = now;
                trusted = Some(TrustedBrowserSession {
                    created_at_ms: session.created_at_ms,
                    last_seen_at_ms: session.last_seen_at_ms,
                });
                break;
            }
        }

        self.write_state(&state)?;
        Ok(trusted)
    }

    fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.control_dir)
            .with_context(|| format!("creating {}", self.control_dir.display()))?;
        fs::set_permissions(&self.control_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 700 {}", self.control_dir.display()))
    }

    fn load_state(&self) -> Result<Option<BrowserTrustState>> {
        let path = self.trust_state_path();
        match fs::read_to_string(&path) {
            Ok(raw) => toml::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))
                .map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn write_state(&self, state: &BrowserTrustState) -> Result<()> {
        let mut body = toml::to_string(state)
            .with_context(|| format!("serializing {}", self.trust_state_path().display()))?;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        write_string_atomic(&self.trust_state_path(), &body, 0o600)
    }

    fn write_pairing_code(&self, code: &str) -> Result<()> {
        write_string_atomic(&self.pairing_code_path(), &format!("{code}\n"), 0o600)
    }
}

fn default_trust_state_version() -> u8 {
    TRUST_STATE_VERSION
}

fn new_pairing_challenge(now: u64) -> (String, PairingChallengeRecord) {
    let code = random_pairing_code();
    let record = PairingChallengeRecord {
        code_hash: hash_secret(&code),
        created_at_ms: now,
        expires_at_ms: now.saturating_add(PAIRING_CODE_TTL_MS),
        failed_attempts: 0,
        locked_until_ms: None,
    };
    (code, record)
}

fn retain_fresh_sessions(state: &mut BrowserTrustState, now: u64) {
    let max_age_ms = SESSION_MAX_AGE_SECONDS.saturating_mul(1000);
    state
        .sessions
        .retain(|session| session.created_at_ms.saturating_add(max_age_ms) > now);
}

fn prune_oldest_sessions(state: &mut BrowserTrustState) {
    state
        .sessions
        .sort_by_key(|session| session.last_seen_at_ms);
    while state.sessions.len() > MAX_TRUSTED_SESSIONS {
        state.sessions.remove(0);
    }
}

fn normalize_pairing_code(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>()
}

fn random_pairing_code() -> String {
    let upper = 10u64.pow(PAIRING_CODE_DIGITS as u32);
    let value = OsRng.gen_range(0..upper);
    format!("{value:0width$}", width = PAIRING_CODE_DIGITS)
}

fn random_hex(bytes_len: usize) -> String {
    let mut bytes = vec![0u8; bytes_len];
    OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"bunzo-browser-control-v1:");
    hasher.update(secret.as_bytes());
    to_hex(&hasher.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn write_string_atomic(path: &Path, contents: &str, mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 700 {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bunzo-control"),
        now_ms()
    ));
    fs::write(&tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("chmod {:o} {}", mode, tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
    Ok(())
}

fn now_ms() -> u64 {
    u64::try_from(crate::ledger::now_ms()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn pairing_challenge_writes_local_code_without_storing_plain_code() {
        let tmp = TempDir::new().unwrap();
        let store = BrowserTrustStore::new(tmp.path().join("control"));

        let challenge = store.ensure_pairing_challenge().unwrap();
        let code = fs::read_to_string(&challenge.code_path).unwrap();
        let code = code.trim();

        assert_eq!(code.len(), PAIRING_CODE_DIGITS);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));

        let trust_state = fs::read_to_string(store.trust_state_path()).unwrap();
        assert!(!trust_state.contains(code));
        assert!(trust_state.contains("code_hash"));
        assert!(challenge.expires_at_ms > 0);
    }

    #[test]
    fn pairing_code_creates_trusted_session_and_rotates_code() {
        let tmp = TempDir::new().unwrap();
        let store = BrowserTrustStore::new(tmp.path().join("control"));
        let challenge = store.ensure_pairing_challenge().unwrap();
        let code = fs::read_to_string(&challenge.code_path).unwrap();

        let token = store.pair_with_code(&code).unwrap();
        assert_eq!(token.len(), SESSION_TOKEN_BYTES * 2);
        assert!(store.trusted_session_token(&token).unwrap().is_some());
        assert!(store
            .trusted_session_token("not-a-token")
            .unwrap()
            .is_none());
        assert!(matches!(
            store.pair_with_code(&code),
            Err(PairingError::InvalidCode)
        ));
    }

    #[test]
    fn failed_pairing_attempts_lock_challenge() {
        let tmp = TempDir::new().unwrap();
        let store = BrowserTrustStore::new(tmp.path().join("control"));
        let challenge = store.ensure_pairing_challenge().unwrap();
        let code = fs::read_to_string(challenge.code_path).unwrap();
        let wrong_code = if code.trim() == "0000000000" {
            "1111111111"
        } else {
            "0000000000"
        };

        for _ in 0..PAIRING_MAX_FAILURES {
            let _ = store.pair_with_code(wrong_code);
        }

        assert!(matches!(
            store.pair_with_code(wrong_code),
            Err(PairingError::Locked { .. })
        ));
    }
}
