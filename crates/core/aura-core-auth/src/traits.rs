//! Contracts implemented by each auth-method crate.

use crate::{
    AuthError, AuthorizationRequest, CallbackParams, InferenceCredential, LinkGrant, LinkedAccount,
    OAuthTokens, ProviderKind, RedirectStrategy, StoredSession,
};
use async_trait::async_trait;

/// An account system that owns credentials on this machine (zOS today).
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Which provider this is.
    fn kind(&self) -> ProviderKind;

    /// Sign in with email and password.
    async fn login(&self, email: &str, password: &str) -> Result<StoredSession, AuthError>;

    /// Best-effort remote sign-out.
    async fn logout(&self, session: &StoredSession);
}

/// A personal plan that supplies inference credentials via OAuth.
///
/// Implementations are stateless protocol adapters; the caller owns the
/// callback listener, attempt state and persistence.
#[async_trait]
pub trait SubscriptionProvider: Send + Sync {
    /// Which provider this is.
    fn kind(&self) -> ProviderKind;

    /// How the provider delivers the authorization code.
    fn redirect(&self) -> RedirectStrategy;

    /// Client id for a new attempt. `previous` is the id issued to an
    /// earlier attempt or to the link being reconnected.
    fn client_id(&self, previous: Option<&str>) -> String;

    /// Browser URL that starts the attempt.
    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, AuthError>;

    /// Exchange the callback's code for tokens.
    async fn exchange(
        &self,
        request: &AuthorizationRequest,
        callback: &CallbackParams,
    ) -> Result<LinkGrant, AuthError>;

    /// Renew `tokens`. Must return [`AuthError::ReauthRequired`] when
    /// the refresh grant is permanently invalid and
    /// [`AuthError::Unavailable`] for transient failures.
    async fn refresh(
        &self,
        account: &LinkedAccount,
        tokens: &OAuthTokens,
    ) -> Result<LinkGrant, AuthError>;

    /// Best-effort remote revocation on unlink.
    async fn revoke(&self, _account: &LinkedAccount, _tokens: &OAuthTokens) {}

    /// Plan tier the account behind `tokens` is on (e.g. `Max 20x`).
    /// Best-effort: `None` when the provider does not say.
    async fn plan_tier(&self, _tokens: &OAuthTokens) -> Option<String> {
        None
    }

    /// Request credentials for model calls.
    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential;

    /// Link with an API key the user pasted instead of an authorization
    /// code, for providers that issue long-lived keys (OpenRouter).
    /// The default refuses: OAuth-only providers have no such key.
    async fn accept_api_key(&self, _key: &str) -> Result<LinkGrant, AuthError> {
        Err(AuthError::Invalid(format!(
            "{} does not take an API key; paste the code its sign-in page shows",
            self.kind().label()
        )))
    }

    /// Whether `pasted` looks like an API key this provider would take
    /// through [`Self::accept_api_key`] rather than an authorization code.
    fn is_api_key(&self, _pasted: &str) -> bool {
        false
    }
}

/// Supplies fresh subscription credentials to the model layer.
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Credentials for the active identity's link to `provider`,
    /// refreshed when close to expiry.
    async fn inference_credential(
        &self,
        provider: ProviderKind,
    ) -> Result<InferenceCredential, AuthError>;
}
