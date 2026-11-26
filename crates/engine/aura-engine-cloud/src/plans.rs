//! Plan tokens for cloud machines: each user's Claude, ChatGPT and
//! OpenRouter tokens as boxd host-bound secrets.
//!
//! Every owner (user) who places agents gets three unattached boxd
//! secrets, one per plan ([`names::plan_secret`]), each bound to its
//! provider's API host. A machine lists only its owner's three names, so
//! it holds only `bxds_` placeholders for that owner's tokens, and boxd's
//! egress proxy puts the real token into requests to that host.
//!
//! [`PlanSync`] keeps the values fresh: it reads each owner's credential
//! through [`PlanCredentials`] (refreshed well before expiry), and
//! rotates the secret only when the token changed. Rotation keeps the
//! placeholder, so running machines use the new token on their next
//! request.
//!
//! ## Invariants
//!
//! - Tokens and refresh tokens never enter a machine; only this hub sees
//!   them, and they are never logged (only a hash is kept).
//! - A plan an owner has not linked holds [`UNLINKED`], so the names
//!   always resolve and a later link reaches running machines.
//! - One owner's failing link backs off alone; others keep syncing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aura_core_auth::{InferenceCredential, Owner, ProviderKind};
use aura_core_protocol::cloud::PlanTemplate;
use aura_core_types::time::now_ms;
use parking_lot::Mutex;

use crate::error::CloudResult;
use crate::names::{self, PLAN_KINDS};
use crate::provider::{HostSecret, VmProvider};

/// Value of the secret of a plan its owner has not linked.
pub const UNLINKED: &str = "unlinked";
/// Refresh tokens that expire within this, so a rotated value is in boxd
/// well before the old one stops working.
pub const PLAN_MARGIN_SECS: i64 = 15 * 60;
/// How often [`PlanSync::tick`] should run.
pub const SYNC_EVERY: Duration = Duration::from_secs(120);
/// Longest backoff after an owner's sync fails.
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// The API host a plan's secret is bound to.
#[must_use]
pub const fn plan_host(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Claude => Some("api.anthropic.com"),
        ProviderKind::Chatgpt => Some("api.openai.com"),
        ProviderKind::OpenRouter => Some("openrouter.ai"),
        ProviderKind::Local | ProviderKind::Zos => None,
    }
}

/// Where the hub reads its users' plan credentials. The app implements
/// it over its credential vault; a web service over its users' store.
#[async_trait]
pub trait PlanCredentials: Send + Sync {
    /// `owner`'s `kind` credential, refreshed if it expires within
    /// `margin_secs`; `Ok(None)` when `owner` has not linked it.
    ///
    /// # Errors
    ///
    /// When the link needs a new sign-in or the refresh failed.
    async fn credential(
        &self,
        owner: &Owner,
        kind: ProviderKind,
        margin_secs: i64,
    ) -> Result<Option<InferenceCredential>, String>;

    /// The plans `owner` has linked and can use.
    fn linked(&self, owner: &Owner) -> Vec<ProviderKind>;

    /// The headers and preamble `kind` requests need (bearer empty).
    fn shape(&self, kind: ProviderKind) -> Option<InferenceCredential>;
}

/// One plan's state for an owner, for the HOST tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStatus {
    /// The plan.
    pub kind: ProviderKind,
    /// Whether its secret holds a token (not [`UNLINKED`]).
    pub linked: bool,
    /// When its secret was last set, Unix milliseconds.
    pub synced_ms: Option<u64>,
    /// The host it is bound to.
    pub host: &'static str,
    /// The last sync error, if the last sync failed.
    pub error: Option<String>,
}

#[derive(Default)]
struct OwnerState {
    /// Hash of the value each secret holds now.
    hashes: HashMap<ProviderKind, [u8; 32]>,
    synced_ms: HashMap<ProviderKind, u64>,
    failures: u32,
    retry_at: Option<Instant>,
    error: Option<String>,
}

/// Keeps every placing owner's plan secrets in boxd.
pub struct PlanSync {
    provider: Arc<dyn VmProvider>,
    credentials: Arc<dyn PlanCredentials>,
    install: String,
    owners: Mutex<HashMap<String, OwnerState>>,
    /// One sync per owner at a time.
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl std::fmt::Debug for PlanSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanSync")
            .field("install", &self.install)
            .finish_non_exhaustive()
    }
}

fn hash(value: &str) -> [u8; 32] {
    *blake3::hash(value.as_bytes()).as_bytes()
}

impl PlanSync {
    /// Plan sync for this install's machines.
    #[must_use]
    pub fn new(
        provider: Arc<dyn VmProvider>,
        credentials: Arc<dyn PlanCredentials>,
        install: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            credentials,
            install: install.into(),
            owners: Mutex::new(HashMap::new()),
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// The three secret names of `owner`, in [`PLAN_KINDS`] order.
    #[must_use]
    pub fn secret_names(&self, owner: &Owner) -> Vec<String> {
        let tag = names::owner_tag(owner);
        PLAN_KINDS
            .iter()
            .filter_map(|kind| names::plan_secret(&tag, &self.install, *kind))
            .collect()
    }

    /// The templates a machine of `owner` gets in its claim: one per
    /// plan, linked or not, so a plan linked later works there at once.
    #[must_use]
    pub fn templates(&self, owner: &Owner) -> Vec<PlanTemplate> {
        let tag = names::owner_tag(owner);
        PLAN_KINDS
            .iter()
            .filter_map(|kind| {
                let env = names::plan_secret(&tag, &self.install, *kind)?;
                let shape = self.credentials.shape(*kind);
                Some(PlanTemplate {
                    provider: kind.id().to_string(),
                    env,
                    headers: shape
                        .as_ref()
                        .map(|s| s.headers.clone())
                        .unwrap_or_default(),
                    system_preamble: shape.and_then(|s| s.system_preamble),
                    placeholder: None,
                })
            })
            .collect()
    }

    /// Make sure `owner`'s three secrets exist and hold current values,
    /// before a machine names them. Returns their names.
    ///
    /// # Errors
    ///
    /// When boxd refuses a secret.
    pub async fn ensure_owner(&self, owner: &Owner) -> CloudResult<Vec<String>> {
        self.sync_owner(owner, true).await?;
        Ok(self.secret_names(owner))
    }

    /// Rotate every listed owner's secrets whose token changed. Owners
    /// that failed recently are skipped until their backoff ends.
    pub async fn tick(&self, owners: &[Owner]) {
        for owner in owners {
            let tag = names::owner_tag(owner);
            let waiting = self
                .owners
                .lock()
                .get(&tag)
                .and_then(|state| state.retry_at)
                .is_some_and(|at| Instant::now() < at);
            if waiting {
                continue;
            }
            if let Err(err) = self.sync_owner(owner, false).await {
                tracing::warn!(owner = %tag, error = %err, "plan secrets sync failed; backing off");
            }
        }
    }

    /// Point `owner`'s `kind` secret at [`UNLINKED`] now (they signed out
    /// of it).
    ///
    /// # Errors
    ///
    /// When boxd refuses the update.
    pub async fn revoke(&self, owner: &Owner, kind: ProviderKind) -> CloudResult<()> {
        let tag = names::owner_tag(owner);
        let gate = self.gate(&tag);
        let _one = gate.lock().await;
        self.set(&tag, kind, UNLINKED).await
    }

    /// Delete `owner`'s secrets and forget them (the user was removed).
    ///
    /// # Errors
    ///
    /// When boxd refuses a removal.
    pub async fn remove_owner(&self, owner: &Owner) -> CloudResult<()> {
        let tag = names::owner_tag(owner);
        for name in self.secret_names(owner) {
            self.provider.remove_secret(&name).await?;
        }
        self.owners.lock().remove(&tag);
        Ok(())
    }

    /// `owner`'s plans as the HOST tab shows them.
    #[must_use]
    pub fn status(&self, owner: &Owner) -> Vec<PlanStatus> {
        let tag = names::owner_tag(owner);
        let owners = self.owners.lock();
        let state = owners.get(&tag);
        let unlinked = hash(UNLINKED);
        PLAN_KINDS
            .iter()
            .filter_map(|kind| {
                Some(PlanStatus {
                    kind: *kind,
                    linked: state
                        .and_then(|s| s.hashes.get(kind))
                        .is_some_and(|h| *h != unlinked),
                    synced_ms: state.and_then(|s| s.synced_ms.get(kind).copied()),
                    host: plan_host(*kind)?,
                    error: state.and_then(|s| s.error.clone()),
                })
            })
            .collect()
    }

    fn gate(&self, tag: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.gates.lock().entry(tag.to_string()).or_default())
    }

    async fn sync_owner(&self, owner: &Owner, strict: bool) -> CloudResult<()> {
        let tag = names::owner_tag(owner);
        let gate = self.gate(&tag);
        let _one = gate.lock().await;
        let linked = self.credentials.linked(owner);
        let mut first_error: Option<String> = None;
        for kind in PLAN_KINDS {
            let value = if linked.contains(&kind) {
                match self
                    .credentials
                    .credential(owner, kind, PLAN_MARGIN_SECS)
                    .await
                {
                    Ok(Some(credential)) => zeroize::Zeroizing::new(credential.bearer),
                    Ok(None) => zeroize::Zeroizing::new(UNLINKED.to_string()),
                    Err(err) => {
                        // Keep whatever the secret holds; a token that is
                        // still valid there keeps working.
                        first_error.get_or_insert(format!("{}: {err}", kind.label()));
                        let known = self
                            .owners
                            .lock()
                            .get(&tag)
                            .is_some_and(|s| s.hashes.contains_key(&kind));
                        if known {
                            continue;
                        }
                        zeroize::Zeroizing::new(UNLINKED.to_string())
                    }
                }
            } else {
                zeroize::Zeroizing::new(UNLINKED.to_string())
            };
            if let Err(err) = self.set(&tag, kind, &value).await {
                self.fail(&tag, err.to_string());
                if strict {
                    return Err(err);
                }
                return Ok(());
            }
        }
        let mut owners = self.owners.lock();
        let state = owners.entry(tag).or_default();
        match first_error {
            Some(error) => {
                state.error = Some(error);
            }
            None => {
                state.error = None;
                state.failures = 0;
                state.retry_at = None;
            }
        }
        Ok(())
    }

    fn fail(&self, tag: &str, error: String) {
        let mut owners = self.owners.lock();
        let state = owners.entry(tag.to_string()).or_default();
        state.failures = state.failures.saturating_add(1);
        let backoff = Duration::from_secs(30)
            .saturating_mul(1 << state.failures.min(6))
            .min(MAX_BACKOFF);
        state.retry_at = Some(Instant::now() + backoff);
        state.error = Some(error);
    }

    /// Set `tag`'s `kind` secret to `value` unless it already holds it.
    async fn set(&self, tag: &str, kind: ProviderKind, value: &str) -> CloudResult<()> {
        let Some(name) = names::plan_secret(tag, &self.install, kind) else {
            return Ok(());
        };
        let Some(host) = plan_host(kind) else {
            return Ok(());
        };
        let digest = hash(value);
        let unchanged = self
            .owners
            .lock()
            .get(tag)
            .and_then(|s| s.hashes.get(&kind))
            .is_some_and(|h| *h == digest);
        if unchanged {
            return Ok(());
        }
        self.provider
            .set_secret(&HostSecret {
                name,
                value: zeroize::Zeroizing::new(value.to_string()),
                domains: vec![host.to_string()],
                replace_in: vec!["headers".to_string()],
            })
            .await?;
        tracing::info!(owner = %tag, plan = kind.id(), linked = value != UNLINKED, "plan secret set");
        let mut owners = self.owners.lock();
        let state = owners.entry(tag.to_string()).or_default();
        state.hashes.insert(kind, digest);
        state.synced_ms.insert(kind, now_ms());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
