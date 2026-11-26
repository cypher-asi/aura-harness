//! Linking and unlinking subscription plans and refreshing their tiers.

use super::{AuthManager, LinkMode, PendingLink};
use crate::error::AuthError;
use crate::identity::Identity;
use aura_auth_kit::{LoopbackListener, CALLBACK_TIMEOUT};
use aura_core_auth::{
    AuthError as ProviderError, LinkGrant, LinkStatus, LinkedAccount, Owner, ProviderKind,
    SubscriptionProvider,
};
use chrono::Utc;
use tracing::{info, warn};

impl AuthManager {
    /// Re-read each usable link's plan tier from its provider, so an
    /// upgrade or a link made before tiers were recorded shows up.
    /// Best-effort: links whose tier can't be read keep the stored one.
    pub async fn refresh_plan_tiers(&self) {
        for provider in self
            .links()
            .into_iter()
            .filter(LinkedAccount::usable)
            .map(|l| l.provider)
        {
            let Ok(adapter) = self.subscription(provider) else {
                continue;
            };
            if self.credential_for(provider).await.is_err() {
                continue;
            }
            let Some(mut link) = self.link(provider) else {
                continue;
            };
            let Some(tokens) = self.vault.tokens(&link) else {
                continue;
            };
            let tier = adapter.plan_tier(&tokens).await;
            if tier.is_some() && tier != link.plan {
                link.plan = tier;
                if let Err(e) = self.vault.update_link(&link) {
                    warn!(provider = %provider, error = %e, "Could not save plan tier");
                }
            }
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
    /// In paste mode, input the provider recognises as an API key (see
    /// [`SubscriptionProvider::is_api_key`]) is checked and stored as the
    /// credential instead of being exchanged as an authorization code.
    ///
    /// # Errors
    ///
    /// Fails on expiry, a declined or invalid callback, a failed code
    /// exchange, a rejected API key, or when the active identity changed
    /// mid-sign-in.
    pub async fn finish_link(
        &self,
        pending: PendingLink,
        pasted: Option<&str>,
    ) -> Result<LinkedAccount, AuthError> {
        let provider = pending.provider;
        let owner = pending.owner.clone();
        let adapter = self.subscription(provider)?;
        let api_key = pasted.filter(|p| pending.needs_paste() && adapter.is_api_key(p));
        let grant = match api_key {
            Some(key) => {
                self.ensure_owner(&owner)?;
                adapter.accept_api_key(key.trim()).await?
            }
            None => {
                self.exchange_code(adapter.as_ref(), pending, pasted)
                    .await?
            }
        };
        self.store_grant(adapter.as_ref(), provider, &owner, grant)
            .await
    }

    /// Link `provider` to the active identity with an API key, without
    /// a browser sign-in. Only providers that issue long-lived keys
    /// (OpenRouter) accept one.
    ///
    /// # Errors
    ///
    /// Fails when the provider is unknown or takes no API key, the key
    /// is malformed or rejected, or on storage failures.
    pub async fn link_api_key(
        &self,
        provider: ProviderKind,
        key: &str,
    ) -> Result<LinkedAccount, AuthError> {
        let adapter = self.subscription(provider)?;
        let owner = self.owner();
        let grant = adapter.accept_api_key(key.trim()).await?;
        self.store_grant(adapter.as_ref(), provider, &owner, grant)
            .await
    }

    /// Wait for (or parse the pasted) authorization code and exchange it.
    pub(super) async fn exchange_code(
        &self,
        adapter: &dyn SubscriptionProvider,
        pending: PendingLink,
        pasted: Option<&str>,
    ) -> Result<LinkGrant, AuthError> {
        let callback = match pending.listener {
            Some(listener) => listener.wait(CALLBACK_TIMEOUT).await?,
            None => aura_auth_kit::parse_pasted_code(
                pasted.unwrap_or_default(),
                &pending.request.state,
            )?,
        };
        self.ensure_owner(&pending.owner)?;
        match adapter.exchange(&pending.request, &callback).await {
            Ok(grant) => Ok(grant),
            Err(e) => {
                let existing = self.vault.link(&pending.owner, pending.provider);
                if existing.is_none() {
                    if let Some(issued) = &callback.client_id {
                        self.vault.set_issued_client(
                            pending.provider,
                            &pending.owner,
                            Some(issued),
                        )?;
                    }
                }
                Err(e.into())
            }
        }
    }

    /// Fails when the active identity is no longer `owner`.
    pub(super) fn ensure_owner(&self, owner: &Owner) -> Result<(), AuthError> {
        if self.owner() != *owner {
            return Err(ProviderError::Invalid(
                "The signed-in account changed during sign-in; start again".into(),
            )
            .into());
        }
        Ok(())
    }

    /// Store `grant` as `owner`'s link to `provider`, replacing any
    /// existing link and keeping its details where the grant has none.
    pub(super) async fn store_grant(
        &self,
        adapter: &dyn SubscriptionProvider,
        provider: ProviderKind,
        owner: &Owner,
        grant: LinkGrant,
    ) -> Result<LinkedAccount, AuthError> {
        let existing = self.vault.link(owner, provider);
        let now = Utc::now();
        let plan = adapter
            .plan_tier(&grant.tokens)
            .await
            .or_else(|| existing.as_ref().and_then(|l| l.plan.clone()));
        let link = LinkedAccount {
            id: uuid::Uuid::new_v4().to_string(),
            provider,
            owner: owner.clone(),
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
            plan,
        };
        self.vault.put_link(&link, &grant.tokens)?;
        self.vault.set_issued_client(provider, owner, None)?;
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
}
