//! The app's [`PlanCredentials`]: users' plans from the credential
//! vault, for the cloud hub's plan secrets.

use std::sync::Arc;

use crate::plans::PlanCredentials;
use async_trait::async_trait;
use aura_auth::AuthManager;
use aura_core_auth::{AuthError, InferenceCredential, Owner, ProviderKind};

/// Plan credentials from the vault, for any user who has linked plans
/// here (not only the one signed in).
pub struct VaultPlans(pub Arc<AuthManager>);

#[async_trait]
impl PlanCredentials for VaultPlans {
    async fn credential(
        &self,
        owner: &Owner,
        kind: ProviderKind,
        margin_secs: i64,
    ) -> Result<Option<InferenceCredential>, String> {
        match self
            .0
            .inference_credential_for(owner, kind, margin_secs)
            .await
        {
            Ok(credential) => Ok(Some(credential)),
            Err(AuthError::NotLinked(_)) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    fn linked(&self, owner: &Owner) -> Vec<ProviderKind> {
        self.0.linked_plans(owner)
    }

    fn shape(&self, kind: ProviderKind) -> Option<InferenceCredential> {
        self.0.credential_shape(kind)
    }
}
