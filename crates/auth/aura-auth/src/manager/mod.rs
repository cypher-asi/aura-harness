//! [`AuthManager`]: composes identity and subscription providers with the vault.

use crate::error::AuthError;
use crate::identity::Identity;
use crate::vault::Vault;
use aura_auth_kit::LoopbackListener;
use aura_core_auth::{
    AuthError as ProviderError, AuthorizationRequest, IdentityProvider, LinkedAccount, Owner,
    ProviderKind, StoredSession, SubscriptionProvider,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    /// Display names of those links' plans, e.g. `Claude Max 20x`.
    pub plan_names: Vec<String>,
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

/// Short plan label for the UI badge. OpenRouter's tier (`free`,
/// `credits`, `$N left`) is billing detail rather than a plan, so the
/// badge shows just the provider name.
fn badge_name(link: &LinkedAccount) -> String {
    match link.provider {
        ProviderKind::OpenRouter => link.provider.label().to_string(),
        _ => link.plan_name(),
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
    /// When each link's last early refresh failed.
    early_failed: Mutex<HashMap<String, Instant>>,
}

impl AuthManager {
    /// Production manager: `~/.aura` vault, zOS, ChatGPT, Claude and
    /// OpenRouter.
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
                .with_subscription(Arc::new(aura_auth_claude::ClaudeProvider::new()))
                .with_subscription(Arc::new(aura_auth_openrouter::OpenRouterProvider::new())),
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
            early_failed: Mutex::new(HashMap::new()),
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
        let usable: Vec<LinkedAccount> = self
            .vault
            .links_for(&identity.owner())
            .into_iter()
            .filter(LinkedAccount::usable)
            .collect();
        AuthSummary {
            zos_user: identity.display_name().map(str::to_owned),
            subscriptions: usable.iter().map(|l| l.provider).collect(),
            plan_names: usable.iter().map(badge_name).collect(),
        }
    }
}

mod credentials;
mod linking;

#[cfg(test)]
mod tests;
