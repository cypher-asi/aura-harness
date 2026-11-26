//! Inference credentials for linked plans, refreshing tokens as needed.

use super::{AuthManager, REFRESH_MARGIN_SECS};
use async_trait::async_trait;
use aura_core_auth::{
    AuthError as ProviderError, CredentialResolver, InferenceCredential, LinkStatus, LinkedAccount,
    OAuthTokens, Owner, ProviderKind, SubscriptionProvider,
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::warn;

impl AuthManager {
    pub(super) fn refresh_lock(&self, link_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.entry(link_id.to_string()).or_default().clone()
    }

    /// `link`'s stored tokens. A keyring that did not answer is a
    /// passing failure, not a sign-out.
    pub(super) fn stored_tokens(&self, link: &LinkedAccount) -> Result<OAuthTokens, ProviderError> {
        let provider = link.provider;
        self.vault
            .read_tokens(link)
            .map_err(|e| {
                ProviderError::Unavailable(format!(
                    "Could not read the {} credentials ({e}); try again",
                    provider.label()
                ))
            })?
            .ok_or_else(|| {
                ProviderError::ReauthRequired(format!(
                    "{} credentials are missing; run `aura login {}`",
                    provider.label(),
                    provider.id()
                ))
            })
    }

    pub(super) async fn credential_for(
        &self,
        provider: ProviderKind,
    ) -> Result<InferenceCredential, ProviderError> {
        self.inference_credential_for(&self.owner(), provider, REFRESH_MARGIN_SECS)
            .await
    }

    /// What a `provider` credential looks like (its headers and system
    /// preamble) with an empty bearer: the template a cloud machine fills
    /// with its placeholder. `None` for non-plan providers.
    #[must_use]
    pub fn credential_shape(&self, provider: ProviderKind) -> Option<InferenceCredential> {
        let adapter = self.subscription(provider).ok()?;
        let tokens = aura_core_auth::OAuthTokens {
            access_token: String::new(),
            refresh_token: None,
            id_token: None,
            scope: String::new(),
            expires_at: Utc::now(),
        };
        Some(adapter.credential(&tokens))
    }

    /// The plans `owner` has linked and can use (not waiting for a new
    /// sign-in, plan usage enabled).
    #[must_use]
    pub fn linked_plans(&self, owner: &Owner) -> Vec<ProviderKind> {
        self.vault
            .links_for(owner)
            .into_iter()
            .filter(|link| self.subscriptions.contains_key(&link.provider))
            .filter(|link| link.plan_enabled)
            .filter(|link| !matches!(link.status, LinkStatus::ReauthRequired { .. }))
            .map(|link| link.provider)
            .collect()
    }

    /// `owner`'s credential for `provider`, refreshed when it expires
    /// within `margin_secs`. Any owner in the vault, not only the one
    /// signed in: the cloud hub keeps every placing user's tokens fresh.
    /// Refreshes run under the link's lock, so one refresh token is never
    /// spent twice.
    ///
    /// # Errors
    ///
    /// [`ProviderError::NotLinked`] when `owner` has no link to it;
    /// [`ProviderError::ReauthRequired`] when it needs a new sign-in.
    pub async fn inference_credential_for(
        &self,
        owner: &Owner,
        provider: ProviderKind,
        margin_secs: i64,
    ) -> Result<InferenceCredential, ProviderError> {
        let adapter = self.subscription(provider)?;
        let owner = owner.clone();
        let link = self
            .vault
            .link(&owner, provider)
            .ok_or(ProviderError::NotLinked(provider))?;
        let lock = self.refresh_lock(&link.id);
        let _guard = lock.lock().await;

        // Re-read under the lock: another task may have just refreshed.
        let link = self.usable_link(&owner, provider)?;
        let mut tokens = self.stored_tokens(&link)?;
        let now = Utc::now();
        if tokens.expires_within(now, margin_secs) {
            // Another process may have refreshed them since they were
            // kept, spending the refresh token held here.
            self.vault.forget_tokens(&link);
            tokens = self.stored_tokens(&link)?;
        }
        if !tokens.expires_within(now, margin_secs) {
            return Ok(adapter.credential(&tokens));
        }
        self.refresh_credential(adapter.as_ref(), link, &tokens, now)
            .await
    }

    /// `owner`'s link to `provider`, unless it needs a new sign-in or its
    /// plan does not allow usage here.
    fn usable_link(
        &self,
        owner: &Owner,
        provider: ProviderKind,
    ) -> Result<LinkedAccount, ProviderError> {
        let link = self
            .vault
            .link(owner, provider)
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
        Ok(link)
    }

    /// Refresh `link`'s expiring `tokens` and return the new credential;
    /// on a transient failure, the unexpired old one.
    async fn refresh_credential(
        &self,
        adapter: &dyn SubscriptionProvider,
        mut link: LinkedAccount,
        tokens: &OAuthTokens,
        now: DateTime<Utc>,
    ) -> Result<InferenceCredential, ProviderError> {
        let provider = link.provider;
        match adapter.refresh(&link, tokens).await {
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
                Ok(adapter.credential(tokens))
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
