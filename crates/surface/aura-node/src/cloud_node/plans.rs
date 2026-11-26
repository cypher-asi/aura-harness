//! The owner's plan credentials and filling boxd placeholders into a claim.

use super::*;

/// The plan credentials of the cloud node's owner, as its claim gave
/// them: each bearer is a boxd `bxds_` placeholder, which boxd's egress
/// proxy swaps for the owner's real token on the way to the provider.
/// Before a claim, and for plans without a placeholder, it answers
/// "not linked".
#[derive(Default)]
pub struct PlanResolver {
    inner: RwLock<(Option<String>, Vec<aura_core_protocol::cloud::PlanTemplate>)>,
}

impl std::fmt::Debug for PlanResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("PlanResolver")
            .field("owner", &inner.0)
            .field("plans", &inner.1.len())
            .finish()
    }
}

impl PlanResolver {
    /// Use `plans` of `owner` from now on.
    pub fn set(&self, owner: Option<String>, plans: &[aura_core_protocol::cloud::PlanTemplate]) {
        let usable: Vec<_> = plans
            .iter()
            .filter(|plan| {
                plan.placeholder
                    .as_deref()
                    .is_some_and(|p| p.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX))
            })
            .cloned()
            .collect();
        tracing::info!(
            owner = owner.as_deref().unwrap_or("-"),
            plans = usable.len(),
            "cloud node plans set"
        );
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = (owner, usable);
    }

    /// The owner tag the plans belong to.
    #[must_use]
    pub fn owner(&self) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .clone()
    }
}

#[async_trait::async_trait]
impl aura_core_auth::CredentialResolver for PlanResolver {
    async fn inference_credential(
        &self,
        provider: aura_core_auth::ProviderKind,
    ) -> Result<aura_core_auth::InferenceCredential, aura_core_auth::AuthError> {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let plan = inner
            .1
            .iter()
            .find(|plan| plan.provider == provider.id())
            .ok_or(aura_core_auth::AuthError::NotLinked(provider))?;
        let bearer = plan
            .placeholder
            .clone()
            .ok_or(aura_core_auth::AuthError::NotLinked(provider))?;
        Ok(aura_core_auth::InferenceCredential {
            provider,
            bearer,
            headers: plan.headers.clone(),
            system_preamble: plan.system_preamble.clone(),
        })
    }
}

/// Fill each plan template's placeholder in `claim` from `lookup` (the
/// machine's environment): boxd puts a `bxds_` placeholder for each of
/// the owner's host-bound secrets there. A plan whose variable is unset
/// stays without one (its requests then fail as not linked).
///
/// # Errors
///
/// When a template names a secret that is not the claim owner's plan
/// secret, or a variable holds anything but a `bxds_` placeholder: a real
/// token must never end up on a machine.
pub fn fill_placeholders(
    claim: &mut NodeClaim,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<usize, String> {
    let owner = claim.owner.clone().unwrap_or_default();
    let mut filled = 0;
    for plan in &mut claim.plans {
        if !aura_engine_cloud::names::is_plan_secret_of(&plan.env, &owner) {
            return Err(format!(
                "plan {} names {}, which is not this machine owner's secret",
                plan.provider, plan.env
            ));
        }
        let Some(value) = lookup(&plan.env).filter(|v| !v.trim().is_empty()) else {
            plan.placeholder = None;
            continue;
        };
        let value = value.trim().to_string();
        if !value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX) {
            return Err(format!(
                "{} holds a value that is not a boxd placeholder; refusing to keep a plan token on this machine",
                plan.env
            ));
        }
        plan.placeholder = Some(value);
        filled += 1;
    }
    if let Some(git) = claim.git.as_mut() {
        git.placeholder = None;
        if let Some(env) = git.env.as_deref() {
            if !aura_engine_cloud::names::is_repo_secret(env) {
                return Err(format!("git names {env}, which is not a repository secret"));
            }
            if let Some(value) = lookup(env).filter(|v| !v.trim().is_empty()) {
                let value = value.trim().to_string();
                if !value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX) {
                    return Err(format!(
                        "{env} holds a value that is not a boxd placeholder; refusing to keep a GitHub token on this machine"
                    ));
                }
                git.placeholder = Some(value);
            }
        }
    }
    Ok(filled)
}

/// Whether `value` is a boxd placeholder that can go in a file line.
pub(super) fn is_placeholder(value: &str) -> bool {
    value.len() > aura_core_protocol::cloud::PLACEHOLDER_PREFIX.len()
        && value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX)
        && value.chars().all(|c| c.is_ascii_graphic())
}

/// `value` as it may sit in a URL's user info.
pub(super) fn url_encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `value` inside a double-quoted YAML string.
pub(super) fn yaml_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
