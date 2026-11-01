//! [`AuthManager`]: composes identity and subscription providers with the vault.

use crate::error::AuthError;
use crate::identity::Identity;
use crate::vault::Vault;
use async_trait::async_trait;
use aura_auth_kit::{LoopbackListener, CALLBACK_TIMEOUT};
use aura_core_auth::{
    AuthError as ProviderError, AuthorizationRequest, CredentialResolver, IdentityProvider,
    InferenceCredential, LinkStatus, LinkedAccount, Owner, ProviderKind, StoredSession,
    SubscriptionProvider,
};
use chrono::Utc;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Refresh access tokens this close to expiry.
const REFRESH_MARGIN_SECS: i64 = 60;

/// How a subscription sign-in returns its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Browser redirects to a local callback listener.
    Browser,
    /// User pastes the code shown by the provider (headless machines).
    Paste,
}

/// An in-progress subscription sign-in.
pub struct PendingLink {
    provider: ProviderKind,
    owner: Owner,
    url: String,
    request: AuthorizationRequest,
    listener: Option<LoopbackListener>,
}

impl PendingLink {
    /// Provider being linked.
    #[must_use]
    pub fn provider(&self) -> ProviderKind {
        self.provider
    }

    /// URL the user must open.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether [`AuthManager::finish_link`] needs pasted input.
    #[must_use]
    pub fn needs_paste(&self) -> bool {
        self.listener.is_none()
    }
}

/// Caveat to show before linking `provider`, if any.
#[must_use]
pub fn sign_in_notice(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Claude => Some(aura_auth_claude::TERMS_NOTICE),
        _ => None,
    }
}

/// Snapshot of what the active identity can use, for UI gating.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthSummary {
    /// zOS display name when signed in.
    pub zos_user: Option<String>,
    /// Subscription providers with a usable link.
    pub subscriptions: Vec<ProviderKind>,
}

impl AuthSummary {
    /// Whether `provider` can serve requests.
    #[must_use]
    pub fn can_use(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Local => true,
            ProviderKind::Zos => self.zos_user.is_some(),
            other => self.subscriptions.contains(&other),
        }
    }
}

/// Registry of auth providers plus persistent state.
///
/// Implements [`CredentialResolver`] so model providers can fetch
/// fresh subscription credentials per request.
pub struct AuthManager {
    vault: Vault,
    zos: Arc<dyn IdentityProvider>,
    subscriptions: BTreeMap<ProviderKind, Arc<dyn SubscriptionProvider>>,
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AuthManager {
    /// Production manager: `~/.aura` vault, zOS, ChatGPT and Claude.
    ///
    /// # Errors
    ///
    /// Fails when the home directory, HTTP client or ChatGPT host id
    /// is unavailable.
    pub fn new() -> Result<Self, AuthError> {
        let vault = Vault::default_location()?;
        let host_id =
            aura_auth_chatgpt::load_or_create_host_id(&vault.root().join("chatgpt-host"))?;
        Ok(
            Self::with_identity(vault, Arc::new(aura_auth_zos::ZosIdentity::new()?))
                .with_subscription(Arc::new(aura_auth_chatgpt::ChatgptProvider::new(host_id)))
                .with_subscription(Arc::new(aura_auth_claude::ClaudeProvider::new())),
        )
    }

    /// Manager with an explicit vault and zOS provider and no
    /// subscriptions; add them with [`Self::with_subscription`].
    #[must_use]
    pub fn with_identity(vault: Vault, zos: Arc<dyn IdentityProvider>) -> Self {
        Self {
            vault,
            zos,
            subscriptions: BTreeMap::new(),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Register a subscription provider, replacing any of the same kind.
    #[must_use]
    pub fn with_subscription(mut self, provider: Arc<dyn SubscriptionProvider>) -> Self {
        self.subscriptions.insert(provider.kind(), provider);
        self
    }

    /// Underlying storage.
    #[must_use]
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// Registered subscription providers, in display order.
    #[must_use]
    pub fn subscription_kinds(&self) -> Vec<ProviderKind> {
        self.subscriptions.keys().copied().collect()
    }

    fn subscription(
        &self,
        kind: ProviderKind,
    ) -> Result<&Arc<dyn SubscriptionProvider>, ProviderError> {
        self.subscriptions.get(&kind).ok_or_else(|| {
            ProviderError::Invalid(format!("{} is not a subscription provider", kind.label()))
        })
    }

    // ---- identity --------------------------------------------------------

    /// The active identity.
    #[must_use]
    pub fn identity(&self) -> Identity {
        self.vault
            .zos_session()
            .map_or(Identity::Local, Identity::Zos)
    }

    /// Owner of links for the active identity.
    #[must_use]
    pub fn owner(&self) -> Owner {
        self.identity().owner()
    }

    /// Sign in to zOS. Links made while local stay local until moved
    /// with [`Self::move_local_links_to_zos`].
    ///
    /// # Errors
    ///
    /// Propagates provider and storage failures.
    pub async fn login_zos(&self, email: &str, password: &str) -> Result<StoredSession, AuthError> {
        let session = self.zos.login(email, password).await?;
        self.vault.save_zos_session(&session)?;
        Ok(session)
    }

    /// Sign out of zOS. The user's links are kept but hidden until the
    /// same user signs in again. Returns the previous session.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub async fn logout_zos(&self) -> Result<Option<StoredSession>, AuthError> {
        let session = self.vault.zos_session();
        if let Some(session) = &session {
            self.zos.logout(session).await;
        }
        self.vault.clear_zos_session()?;
        Ok(session)
    }

    // ---- subscriptions ---------------------------------------------------

    /// Links owned by the active identity.
    #[must_use]
    pub fn links(&self) -> Vec<LinkedAccount> {
        self.vault.links_for(&self.owner())
    }

    /// The active identity's link to `provider`.
    #[must_use]
    pub fn link(&self, provider: ProviderKind) -> Option<LinkedAccount> {
        self.vault.link(&self.owner(), provider)
    }

    /// What the active identity can use right now.
    #[must_use]
    pub fn summary(&self) -> AuthSummary {
        let identity = self.identity();
        let subscriptions = self
            .vault
            .links_for(&identity.owner())
            .into_iter()
            .filter(LinkedAccount::usable)
            .map(|l| l.provider)
            .collect();
        AuthSummary {
            zos_user: identity.display_name().map(str::to_owned),
            subscriptions,
        }
    }

    /// Start linking `provider` to the active identity.
    ///
    /// [`LinkMode::Browser`] uses the provider's loopback redirect when
    /// it has one; otherwise (or with [`LinkMode::Paste`]) the
    /// provider's manual-code page is used.
    ///
    /// # Errors
    ///
    /// Fails when the provider is unknown, supports neither redirect
    /// mode, or the callback listener cannot bind.
    pub async fn begin_link(
        &self,
        provider: ProviderKind,
        mode: LinkMode,
    ) -> Result<PendingLink, AuthError> {
        let adapter = self.subscription(provider)?;
        let owner = self.owner();
        let existing = self.vault.link(&owner, provider);
        let previous_client = existing
            .as_ref()
            .map(|l| l.client_id.clone())
            .or_else(|| self.vault.issued_client(provider, &owner));
        let client_id = adapter.client_id(previous_client.as_deref());
        let login_hint = existing.map(|l| l.label).filter(|l| !l.is_empty());
        let strategy = adapter.redirect();

        let (listener, request) = match (mode, strategy.loopback, strategy.manual) {
            (LinkMode::Browser, Some(loopback), _) | (LinkMode::Paste, Some(loopback), None) => {
                let mut request = aura_auth_kit::new_attempt(client_id, String::new(), login_hint);
                let listener = LoopbackListener::bind(loopback, request.state.clone()).await?;
                request.redirect_uri = listener.redirect_uri().to_string();
                (Some(listener), request)
            }
            (_, _, Some(manual)) => (
                None,
                aura_auth_kit::new_attempt(client_id, manual.to_string(), login_hint),
            ),
            (_, None, None) => {
                return Err(ProviderError::Invalid(format!(
                    "{} has no supported sign-in redirect",
                    provider.label()
                ))
                .into())
            }
        };
        let url = adapter.authorization_url(&request)?;
        Ok(PendingLink {
            provider,
            owner,
            url,
            request,
            listener,
        })
    }

    /// Finish a sign-in started by [`Self::begin_link`], storing the
    /// link under the identity that started it.
    ///
    /// # Errors
    ///
    /// Fails on expiry, a declined or invalid callback, a failed code
    /// exchange, or when the active identity changed mid-sign-in.
    pub async fn finish_link(
        &self,
        pending: PendingLink,
        pasted: Option<&str>,
    ) -> Result<LinkedAccount, AuthError> {
        let adapter = self.subscription(pending.provider)?;
        let callback = match pending.listener {
            Some(listener) => listener.wait(CALLBACK_TIMEOUT).await?,
            None => aura_auth_kit::parse_pasted_code(
                pasted.unwrap_or_default(),
                &pending.request.state,
            )?,
        };
        if self.owner() != pending.owner {
            return Err(ProviderError::Invalid(
                "The signed-in account changed during sign-in; start again".into(),
            )
            .into());
        }
        let existing = self.vault.link(&pending.owner, pending.provider);
        let grant = match adapter.exchange(&pending.request, &callback).await {
            Ok(grant) => grant,
            Err(e) => {
                if existing.is_none() {
                    if let Some(issued) = &callback.client_id {
                        self.vault.set_issued_client(
                            pending.provider,
                            &pending.owner,
                            Some(issued),
                        )?;
                    }
                }
                return Err(e.into());
            }
        };
        let now = Utc::now();
        let link = LinkedAccount {
            id: uuid::Uuid::new_v4().to_string(),
            provider: pending.provider,
            owner: pending.owner.clone(),
            subject: grant
                .subject
                .or_else(|| existing.as_ref().map(|l| l.subject.clone()))
                .unwrap_or_default(),
            label: grant
                .label
                .or_else(|| existing.as_ref().map(|l| l.label.clone()))
                .unwrap_or_default(),
            client_id: grant.client_id,
            plan_enabled: grant.plan_enabled,
            expires_at: grant.tokens.expires_at,
            created_at: now,
            status: LinkStatus::Active,
        };
        self.vault.put_link(&link, &grant.tokens)?;
        self.vault
            .set_issued_client(pending.provider, &pending.owner, None)?;
        info!(provider = %link.provider, owner = %link.owner, "Subscription linked");
        Ok(link)
    }

    /// Unlink `provider` from the active identity, revoking remotely
    /// when supported. Returns the removed link.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub async fn unlink(&self, provider: ProviderKind) -> Result<Option<LinkedAccount>, AuthError> {
        let Some(link) = self.link(provider) else {
            return Ok(None);
        };
        if let (Ok(adapter), Some(tokens)) = (self.subscription(provider), self.vault.tokens(&link))
        {
            adapter.revoke(&link, &tokens).await;
        }
        self.vault.remove_link(&link)?;
        Ok(Some(link))
    }

    /// Move links made while signed out onto the signed-in zOS user.
    ///
    /// # Errors
    ///
    /// Fails when not signed in to zOS, or on storage failures.
    pub fn move_local_links_to_zos(&self) -> Result<usize, AuthError> {
        let Identity::Zos(session) = self.identity() else {
            return Err(
                ProviderError::Invalid("Sign in to zOS first: `aura login zos`".into()).into(),
            );
        };
        self.vault.move_links(&Owner::Local, &session.owner())
    }

    fn refresh_lock(&self, link_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.entry(link_id.to_string()).or_default().clone()
    }

    async fn credential_for(
        &self,
        provider: ProviderKind,
    ) -> Result<InferenceCredential, ProviderError> {
        let adapter = self.subscription(provider)?;
        let owner = self.owner();
        let link = self
            .vault
            .link(&owner, provider)
            .ok_or(ProviderError::NotLinked(provider))?;
        let lock = self.refresh_lock(&link.id);
        let _guard = lock.lock().await;

        // Re-read under the lock: another task may have just refreshed.
        let mut link = self
            .vault
            .link(&owner, provider)
            .ok_or(ProviderError::NotLinked(provider))?;
        if let LinkStatus::ReauthRequired { reason } = &link.status {
            return Err(ProviderError::ReauthRequired(reason.clone()));
        }
        if !link.plan_enabled {
            return Err(ProviderError::ReauthRequired(format!(
                "Your {} plan does not allow usage here; check your plan, then run `aura login {}`",
                provider.label(),
                provider.id()
            )));
        }
        let tokens = self.vault.tokens(&link).ok_or_else(|| {
            ProviderError::ReauthRequired(format!(
                "{} credentials are missing; run `aura login {}`",
                provider.label(),
                provider.id()
            ))
        })?;
        let now = Utc::now();
        if !tokens.expires_within(now, REFRESH_MARGIN_SECS) {
            return Ok(adapter.credential(&tokens));
        }

        match adapter.refresh(&link, &tokens).await {
            Ok(grant) => {
                // Persist the rotated refresh token before anything can use it.
                self.vault
                    .save_tokens(&link, &grant.tokens)
                    .map_err(ProviderError::from)?;
                link.expires_at = grant.tokens.expires_at;
                link.plan_enabled = grant.plan_enabled;
                self.vault.update_link(&link).map_err(ProviderError::from)?;
                if !grant.plan_enabled {
                    return Err(ProviderError::ReauthRequired(format!(
                        "{} plan usage is no longer enabled",
                        provider.label()
                    )));
                }
                Ok(adapter.credential(&grant.tokens))
            }
            Err(ProviderError::ReauthRequired(reason)) => {
                link.status = LinkStatus::ReauthRequired {
                    reason: reason.clone(),
                };
                let _ = self.vault.update_link(&link);
                let _ = self.vault.clear_tokens(&link);
                Err(ProviderError::ReauthRequired(reason))
            }
            Err(e) if !tokens.expires_within(now, 0) => {
                warn!(provider = %provider, error = %e, "Refresh failed; using unexpired token");
                Ok(adapter.credential(&tokens))
            }
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl CredentialResolver for AuthManager {
    async fn inference_credential(
        &self,
        provider: ProviderKind,
    ) -> Result<InferenceCredential, ProviderError> {
        self.credential_for(provider).await
    }
}

#[cfg(test)]
mod tests;
