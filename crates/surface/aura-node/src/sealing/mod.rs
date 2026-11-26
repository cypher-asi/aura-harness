//! Attestation boot flow: fetch the per-agent state DEK before serving.
//!
//! Swarm TEE upgrade phase 5 ("attest-boot"). When the harness runs as a
//! confidential SEV-SNP CoCo VM, the aura-swarm control plane provisions a
//! random 256-bit data encryption key (DEK) in the Trustee KBS under
//! `swarm/agents/{agent_id}/state-key` and starts the pod with:
//!
//! * `AURA_STATE_ENCRYPTION=sealed`
//! * `AURA_STATE_KEY_ID=swarm/agents/{agent_id}/state-key`
//! * `AURA_KBS_URL=...` (informational; the in-guest fetch goes through
//!   the local confidential-data-hub, which performs the RCAR attestation
//!   handshake with the KBS transparently)
//!
//! On startup, **before opening or serving any state**, `Node::run` calls
//! [`prepare_state_sealing`]:
//!
//! 1. If sealed mode is not requested **and no swarm control-plane env is
//!    present**, return `None` — the plaintext path survives for pure
//!    local/dev usage only.
//! 2. Otherwise fetch the DEK via a [`DekProvider`] — the CoCo CDH
//!    resource endpoint in production ([`CdhDekProvider`]) or a local key
//!    file in dev mode ([`LocalKeyFileDekProvider`]) — retrying with
//!    backoff because the CDH/KBS may not be ready immediately after VM
//!    boot.
//! 3. If no DEK is obtained within the bounded window, startup **fails
//!    hard**. Sealed mode never silently falls back to plaintext.
//!
//! # Swarm agents never run plaintext (TEE upgrade R3)
//!
//! When the process runs as a swarm agent — detected by the control-plane
//! environment the scheduler always injects (`CONTROL_PLANE_URL` /
//! `AGENT_ID`) — plaintext state is **refused**:
//!
//! * `AURA_STATE_ENCRYPTION` unset defaults to `sealed` (every swarm pod
//!   has been started with `sealed` since R1; this default only matters
//!   for hand-rolled pods).
//! * An explicit `AURA_STATE_ENCRYPTION=plaintext`/`none` fails startup.
//!
//! Local dev without those env vars keeps working exactly as before:
//! plaintext by default, or the local-keyfile sealed mode when
//! `AURA_STATE_ENCRYPTION=sealed` + `AURA_STATE_KEY_FILE` are set.
//!
//! Key material is held in [`Zeroizing`] buffers and never logged.
//!
//! # Key id → CDH resource path mapping
//!
//! Mirrors the aura-swarm control plane exactly (`kbs.rs` there): KBS
//! resource paths have three segments (`{repository}/{type}/{tag}`) while
//! the key id has four, so the first segment maps to the repository, the
//! last to the tag, and the middle segments join with `.` to form the
//! type. `swarm/agents/{id}/state-key` is fetched from
//! `{AURA_CDH_URL}/cdh/resource/swarm/agents.{id}/state-key`.

mod providers;
#[cfg(test)]
mod tests;

#[cfg(test)]
use providers::decode_dek;
pub use providers::{kbs_resource_path, CdhDekProvider, LocalKeyFileDekProvider};

use anyhow::{bail, Context};
use async_trait::async_trait;
use aura_store_db::SealCipher;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use zeroize::Zeroizing;

/// Requests sealed state-at-rest when set to `sealed`.
pub const ENV_STATE_ENCRYPTION: &str = "AURA_STATE_ENCRYPTION";
/// Deterministic per-agent DEK key id, e.g. `swarm/agents/{id}/state-key`.
pub const ENV_STATE_KEY_ID: &str = "AURA_STATE_KEY_ID";
/// Base URL of the in-guest confidential-data-hub resource endpoint.
pub const ENV_CDH_URL: &str = "AURA_CDH_URL";
/// Dev-mode key file path; presence selects [`LocalKeyFileDekProvider`].
pub const ENV_STATE_KEY_FILE: &str = "AURA_STATE_KEY_FILE";
/// Bound (seconds) on the total DEK fetch retry window.
pub const ENV_DEK_FETCH_TIMEOUT_SECS: &str = "AURA_DEK_FETCH_TIMEOUT_SECS";
/// Swarm control-plane URL the scheduler injects into every agent pod.
/// Its presence marks the process as a swarm agent (plaintext refused).
pub const ENV_CONTROL_PLANE_URL: &str = "CONTROL_PLANE_URL";
/// Swarm agent id the scheduler injects into every agent pod.
/// Its presence marks the process as a swarm agent (plaintext refused).
pub const ENV_AGENT_ID: &str = "AGENT_ID";

/// CoCo guests ship the CDH on this local endpoint.
pub const DEFAULT_CDH_URL: &str = "http://127.0.0.1:8006";
/// Default bound on the DEK fetch retry window. Generous because the
/// attestation-agent / CDH may come up after the harness container.
pub const DEFAULT_DEK_FETCH_TIMEOUT_SECS: u64 = 120;

/// Non-secret marker file written in the state dir on sealed boot so later
/// phases (encrypt-in-place migration, R2) can detect sealed vs plaintext
/// state without attempting a decrypt.
pub const SEALED_MARKER_FILENAME: &str = ".aura-sealed";

/// Resolved sealing configuration (from env in production).
#[derive(Debug, Clone)]
pub struct SealingConfig {
    /// Whether sealed mode was requested via [`ENV_STATE_ENCRYPTION`].
    pub enabled: bool,
    /// DEK key id (required for the CDH provider; recorded in the marker).
    pub key_id: Option<String>,
    /// Dev-mode key file path; presence selects the local provider.
    pub key_file: Option<PathBuf>,
    /// CDH base URL.
    pub cdh_url: String,
    /// Total retry window before refusing to start.
    pub fetch_timeout: Duration,
}

impl SealingConfig {
    /// Load from process environment.
    ///
    /// # Errors
    /// Returns an error when [`ENV_STATE_ENCRYPTION`] is set to an
    /// unrecognized value — a typo must refuse to start rather than
    /// silently run plaintext — or when plaintext is requested while the
    /// swarm control-plane env ([`ENV_CONTROL_PLANE_URL`] /
    /// [`ENV_AGENT_ID`]) is present (swarm agents never run plaintext).
    pub fn from_env() -> anyhow::Result<Self> {
        let env_present = |name: &str| std::env::var(name).is_ok_and(|v| !v.trim().is_empty());
        let encryption = std::env::var(ENV_STATE_ENCRYPTION).ok();
        let key_id = std::env::var(ENV_STATE_KEY_ID).ok();
        let key_file = std::env::var(ENV_STATE_KEY_FILE).ok();
        let cdh_url = std::env::var(ENV_CDH_URL).ok();
        let timeout_secs = std::env::var(ENV_DEK_FETCH_TIMEOUT_SECS).ok();
        Self::resolve(
            encryption.as_deref(),
            key_id.as_deref(),
            key_file.as_deref(),
            cdh_url.as_deref(),
            timeout_secs.as_deref(),
            env_present(ENV_CONTROL_PLANE_URL) || env_present(ENV_AGENT_ID),
        )
    }

    /// Pure resolution from raw env values (unit-testable without
    /// touching the process environment).
    ///
    /// `swarm_env` says whether the swarm control-plane environment is
    /// present (TEE upgrade R3): then sealed is the default and an
    /// explicit plaintext request is refused. Without it, plaintext
    /// remains the local/dev default.
    ///
    /// # Errors
    /// See [`Self::from_env`].
    pub fn resolve(
        encryption: Option<&str>,
        key_id: Option<&str>,
        key_file: Option<&str>,
        cdh_url: Option<&str>,
        timeout_secs: Option<&str>,
        swarm_env: bool,
    ) -> anyhow::Result<Self> {
        let enabled = match encryption.map(str::trim) {
            None | Some("") => {
                if swarm_env {
                    info!(
                        "swarm control-plane env present and {ENV_STATE_ENCRYPTION} unset; \
                         defaulting to sealed (swarm agents never run plaintext)"
                    );
                }
                swarm_env
            }
            Some(v) if v.eq_ignore_ascii_case("sealed") => true,
            Some(v) if v.eq_ignore_ascii_case("plaintext") || v.eq_ignore_ascii_case("none") => {
                if swarm_env {
                    bail!(
                        "{ENV_STATE_ENCRYPTION}=`{v}` requested but the swarm \
                         control-plane env ({ENV_CONTROL_PLANE_URL}/{ENV_AGENT_ID}) is \
                         present; swarm agents never run with plaintext state (TEE \
                         upgrade R3). Unset {ENV_STATE_ENCRYPTION} or set it to `sealed`."
                    );
                }
                false
            }
            Some(other) => bail!(
                "unrecognized {ENV_STATE_ENCRYPTION} value `{other}` \
                 (expected `sealed`, `plaintext`, or unset); refusing to guess"
            ),
        };

        let fetch_timeout = match timeout_secs.map(str::trim) {
            None | Some("") => Duration::from_secs(DEFAULT_DEK_FETCH_TIMEOUT_SECS),
            Some(raw) => Duration::from_secs(
                raw.parse::<u64>()
                    .with_context(|| format!("invalid {ENV_DEK_FETCH_TIMEOUT_SECS}: `{raw}`"))?,
            ),
        };

        Ok(Self {
            enabled,
            key_id: key_id
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            key_file: key_file
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            cdh_url: cdh_url
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_CDH_URL.to_string()),
            fetch_timeout,
        })
    }

    /// Select the DEK provider for this configuration.
    ///
    /// Dev mode (key file configured) wins over the CDH so local runs
    /// never depend on an attestation stack; otherwise the CoCo CDH
    /// endpoint is used and a key id is required.
    ///
    /// # Errors
    /// Returns an error when sealed mode is requested without either a
    /// key file or a key id.
    pub fn build_provider(&self) -> anyhow::Result<Box<dyn DekProvider>> {
        if let Some(key_file) = &self.key_file {
            return Ok(Box::new(LocalKeyFileDekProvider::new(key_file.clone())));
        }
        let key_id = self.key_id.as_deref().with_context(|| {
            format!(
                "{ENV_STATE_ENCRYPTION}=sealed requires {ENV_STATE_KEY_ID} \
                 (or {ENV_STATE_KEY_FILE} for dev mode)"
            )
        })?;
        Ok(Box::new(CdhDekProvider::new(&self.cdh_url, key_id)?))
    }
}

/// Source of the 256-bit state DEK.
///
/// Production uses [`CdhDekProvider`]; dev mode and tests inject
/// [`LocalKeyFileDekProvider`] or a bespoke mock.
#[async_trait]
pub trait DekProvider: Send + Sync {
    /// Fetch (or, in dev mode, lazily create) the DEK.
    ///
    /// # Errors
    /// Returns an error when the key cannot be obtained; callers retry
    /// via [`fetch_dek_with_retry`].
    async fn fetch_dek(&self) -> anyhow::Result<Zeroizing<[u8; 32]>>;

    /// Human-readable description for logs (must not contain secrets).
    fn describe(&self) -> String;
}

/// Initial retry backoff after a failed DEK fetch.
const RETRY_BACKOFF_INITIAL: Duration = Duration::from_millis(250);
/// Backoff cap between DEK fetch attempts.
const RETRY_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// Fetch the DEK, retrying with exponential backoff until `total_timeout`
/// elapses (the CDH/KBS may not be ready immediately after VM boot).
///
/// # Errors
/// Returns the last fetch error once the window is exhausted. Never falls
/// back to plaintext.
pub async fn fetch_dek_with_retry(
    provider: &dyn DekProvider,
    total_timeout: Duration,
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let started = Instant::now();
    let mut backoff = RETRY_BACKOFF_INITIAL;
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        match provider.fetch_dek().await {
            Ok(dek) => {
                info!(
                    provider = %provider.describe(),
                    attempt,
                    "state DEK obtained"
                );
                return Ok(dek);
            }
            Err(err) => {
                let elapsed = started.elapsed();
                if elapsed >= total_timeout {
                    return Err(err.context(format!(
                        "no DEK obtainable from {} after {attempt} attempt(s) over {:.1}s; \
                         refusing to serve with sealed state requested",
                        provider.describe(),
                        elapsed.as_secs_f64()
                    )));
                }
                // The error chain never contains key material — only
                // transport/status context.
                warn!(
                    provider = %provider.describe(),
                    attempt,
                    error = %format!("{err:#}"),
                    retry_in_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX),
                    "DEK fetch failed; retrying"
                );
                let remaining = total_timeout.saturating_sub(elapsed);
                tokio::time::sleep(backoff.min(remaining)).await;
                backoff = (backoff * 2).min(RETRY_BACKOFF_CAP);
            }
        }
    }
}

/// Contents of the non-secret `.aura-sealed` marker file.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SealedMarker {
    /// Sealed on-disk format version (see `aura_store_db::seal`).
    pub version: u8,
    /// AEAD algorithm identifier.
    pub cipher: String,
    /// DEK key id the state is sealed under (non-secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
}

/// Write the `.aura-sealed` marker into `data_dir` (idempotent).
///
/// # Errors
/// Returns an error if the directory or file cannot be written.
pub fn write_sealed_marker(data_dir: &Path, key_id: Option<&str>) -> anyhow::Result<()> {
    let marker = SealedMarker {
        version: aura_store_db::SEAL_VERSION,
        cipher: "aes-256-gcm".to_string(),
        key_id: key_id.map(str::to_string),
    };
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating state dir {}", data_dir.display()))?;
    let path = data_dir.join(SEALED_MARKER_FILENAME);
    std::fs::write(&path, serde_json::to_vec_pretty(&marker)?)
        .with_context(|| format!("writing sealed marker {}", path.display()))?;
    Ok(())
}

/// Resolve sealed mode from the environment and, when requested, fetch the
/// DEK (with bounded retry), run the encrypt-in-place state migration if
/// this sealed boot found plaintext state at `db_path` (Swarm TEE R2; see
/// [`crate::state_migration`]), and build the value cipher. Returns `None`
/// in plaintext mode. Called by `Node::run` **before** the store is opened.
///
/// # Errors
/// Fails (and the node refuses to serve) when sealed mode is requested but
/// no DEK is obtainable within the bounded window, or when the state
/// migration fails (plaintext state is preserved for a retry).
pub async fn prepare_state_sealing(
    data_dir: &Path,
    db_path: &Path,
) -> anyhow::Result<Option<Arc<SealCipher>>> {
    let config = SealingConfig::from_env()?;
    prepare_with_config(&config, data_dir, db_path).await
}

/// Inner body of [`prepare_state_sealing`], parameterized for tests.
///
/// # Errors
/// See [`prepare_state_sealing`].
pub async fn prepare_with_config(
    config: &SealingConfig,
    data_dir: &Path,
    db_path: &Path,
) -> anyhow::Result<Option<Arc<SealCipher>>> {
    if !config.enabled {
        return Ok(None);
    }

    let provider = config.build_provider()?;
    info!(
        provider = %provider.describe(),
        timeout_secs = config.fetch_timeout.as_secs(),
        "sealed state requested; fetching DEK before serving"
    );

    let dek = fetch_dek_with_retry(provider.as_ref(), config.fetch_timeout).await?;
    let cipher = Arc::new(SealCipher::new(&dek));
    drop(dek);

    // R2 encrypt-in-place: a legacy agent's first sealed boot finds
    // plaintext state — convert it (atomic + crash-resumable) before
    // the store opens. Fresh and already-sealed state dirs are no-ops.
    // Synchronous on purpose: nothing serves until this completes.
    let outcome = crate::state_migration::migrate_state_if_needed(
        data_dir,
        db_path,
        config.key_id.as_deref(),
        &cipher,
    )
    .context("encrypt-in-place state migration (plaintext state preserved; retry next boot)")?;
    if let crate::state_migration::MigrationOutcome::Migrated(stats) = outcome {
        info!(
            values_sealed = stats.values_sealed,
            values_copied = stats.values_copied,
            "plaintext state migrated to sealed format on first sealed boot"
        );
    }

    write_sealed_marker(data_dir, config.key_id.as_deref())?;
    info!("sealed state-at-rest enabled (AES-256-GCM)");
    Ok(Some(cipher))
}
