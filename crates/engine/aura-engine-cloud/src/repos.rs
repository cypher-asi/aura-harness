//! Repository tokens for cloud machines: one boxd host-bound secret per
//! GitHub repository cloud agents work on.
//!
//! Each repository gets an unattached boxd secret ([`names::repo_secret`])
//! bound to `github.com` and `api.github.com`, holding a GitHub App
//! installation token for that repository alone ([`crate::github`]). A
//! machine lists its project's repository secret, so git and `gh` there
//! hold only a `bxds_` placeholder and boxd's egress proxy puts the real
//! token into requests to GitHub.
//!
//! [`RepoSync`] keeps the values fresh: installation tokens last an hour,
//! and [`RepoSync::tick`] re-mints one when it expires within
//! [`REPO_MARGIN_SECS`]. Rotation keeps the placeholder, so running
//! machines use the new token on their next request.
//!
//! ## Invariants
//!
//! - Tokens never enter a machine; only this hub sees them, and they are
//!   never logged (only a hash is kept).
//! - A repository the App can't reach (not configured here, not
//!   installed there) holds [`UNLINKED`], so the name always resolves
//!   and a later install reaches running machines on a later tick.
//! - One repository's failing mint backs off alone; others keep syncing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_core_types::time::now_ms;
use parking_lot::Mutex;

use crate::error::CloudResult;
use crate::github::{BotIdentity, GithubError, RepoRef, RepoTokens};
use crate::names;
use crate::provider::{HostSecret, VmProvider};

/// Value of the secret of a repository the App can't reach.
pub const UNLINKED: &str = crate::plans::UNLINKED;
/// Re-mint tokens that expire within this, so a fresh one is in boxd
/// well before the old one stops working.
pub const REPO_MARGIN_SECS: i64 = 15 * 60;
/// The hosts a repository secret is sent to: git over https, and the
/// REST API `gh` calls.
pub const REPO_HOSTS: [&str; 2] = ["github.com", "api.github.com"];
/// Longest backoff after a repository's mint fails. Shorter than plans':
/// installing the App is a user action whose result should show soon.
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// Shown when no App is configured here.
const NOT_CONFIGURED: &str =
    "no GitHub App is configured here (AURA_GITHUB_APP_ID, AURA_GITHUB_APP_PRIVATE_KEY_PATH)";

/// One repository's state, for the HOST tab and notices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
    /// The repository.
    pub repo: RepoRef,
    /// Its secret's name.
    pub secret: String,
    /// Whether the secret holds a token (not [`UNLINKED`]).
    pub linked: bool,
    /// When that token expires, Unix milliseconds.
    pub expires_at_ms: Option<u64>,
    /// When the secret was last set, Unix milliseconds.
    pub synced_ms: Option<u64>,
    /// Why the last sync gave no token, if it gave none.
    pub error: Option<String>,
    /// Where to install the App, when it isn't installed on the repo.
    pub install_url: Option<String>,
}

#[derive(Default)]
struct RepoState {
    /// Hash of the value the secret holds now.
    hash: Option<[u8; 32]>,
    expires_at_ms: Option<u64>,
    synced_ms: Option<u64>,
    failures: u32,
    retry_at: Option<Instant>,
    error: Option<String>,
    install_url: Option<String>,
}

/// Keeps every placed project's repository secret in boxd.
pub struct RepoSync {
    provider: Arc<dyn VmProvider>,
    /// `None` when no App is configured: every secret holds [`UNLINKED`].
    tokens: Option<Arc<dyn RepoTokens>>,
    install: String,
    repos: Mutex<HashMap<RepoRef, RepoState>>,
    /// One sync per repository at a time.
    gates: Mutex<HashMap<RepoRef, Arc<tokio::sync::Mutex<()>>>>,
}

impl std::fmt::Debug for RepoSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoSync")
            .field("install", &self.install)
            .field("configured", &self.tokens.is_some())
            .finish_non_exhaustive()
    }
}

fn hash(value: &str) -> [u8; 32] {
    *blake3::hash(value.as_bytes()).as_bytes()
}

impl RepoSync {
    /// Repository sync for this install's machines, minting with
    /// `tokens` (`None`: no App configured).
    #[must_use]
    pub fn new(
        provider: Arc<dyn VmProvider>,
        tokens: Option<Arc<dyn RepoTokens>>,
        install: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            tokens,
            install: install.into(),
            repos: Mutex::new(HashMap::new()),
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a GitHub App is configured here.
    #[must_use]
    pub fn configured(&self) -> bool {
        self.tokens.is_some()
    }

    /// The secret name of `repo`.
    #[must_use]
    pub fn secret_name(&self, repo: &RepoRef) -> String {
        names::repo_secret(&self.install, &repo.full_name())
    }

    /// The App's bot, for a machine's git identity; `None` without an
    /// App or when GitHub can't be asked.
    pub async fn identity(&self) -> Option<BotIdentity> {
        match self.tokens.as_ref()?.identity().await {
            Ok(identity) => Some(identity),
            Err(err) => {
                tracing::warn!(error = %err, "could not look up the GitHub App's bot");
                None
            }
        }
    }

    /// Make sure `repo`'s secret exists and holds a current value, before
    /// a machine names it. A repository the App can't reach still gets
    /// its secret (holding [`UNLINKED`]); the status says why.
    ///
    /// # Errors
    ///
    /// When boxd refuses the secret.
    pub async fn ensure(&self, repo: &RepoRef) -> CloudResult<RepoStatus> {
        self.sync(repo, true).await?;
        Ok(self.status(repo).unwrap_or_else(|| RepoStatus {
            repo: repo.clone(),
            secret: self.secret_name(repo),
            linked: false,
            expires_at_ms: None,
            synced_ms: None,
            error: None,
            install_url: None,
        }))
    }

    /// Re-mint every listed repository's token that expires within
    /// [`REPO_MARGIN_SECS`]. Repositories that failed recently are
    /// skipped until their backoff ends.
    pub async fn tick(&self, repos: &[RepoRef]) {
        for repo in repos {
            let waiting = self
                .repos
                .lock()
                .get(repo)
                .and_then(|state| state.retry_at)
                .is_some_and(|at| Instant::now() < at);
            if waiting {
                continue;
            }
            if let Err(err) = self.sync(repo, false).await {
                tracing::warn!(%repo, error = %err, "repo secret sync failed; backing off");
            }
        }
    }

    /// `repo`'s state, once it has been synced.
    #[must_use]
    pub fn status(&self, repo: &RepoRef) -> Option<RepoStatus> {
        let repos = self.repos.lock();
        let state = repos.get(repo)?;
        Some(RepoStatus {
            repo: repo.clone(),
            secret: self.secret_name(repo),
            linked: state.hash.is_some_and(|h| h != hash(UNLINKED)),
            expires_at_ms: state.expires_at_ms,
            synced_ms: state.synced_ms,
            error: state.error.clone(),
            install_url: state.install_url.clone(),
        })
    }

    fn gate(&self, repo: &RepoRef) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.gates.lock().entry(repo.clone()).or_default())
    }

    /// Whether `repo`'s secret holds a token good for longer than the
    /// margin.
    fn fresh(&self, repo: &RepoRef) -> bool {
        let margin_ms = u64::try_from(REPO_MARGIN_SECS).unwrap_or(0) * 1000;
        self.repos.lock().get(repo).is_some_and(|state| {
            state.hash.is_some_and(|h| h != hash(UNLINKED))
                && state
                    .expires_at_ms
                    .is_some_and(|at| at > now_ms().saturating_add(margin_ms))
        })
    }

    async fn sync(&self, repo: &RepoRef, strict: bool) -> CloudResult<()> {
        let gate = self.gate(repo);
        let _one = gate.lock().await;
        if self.fresh(repo) {
            return Ok(());
        }
        let Some(tokens) = self.tokens.as_ref() else {
            self.set(repo, UNLINKED, None).await.inspect_err(|err| {
                self.fail(repo, err.to_string(), None);
            })?;
            self.note(repo, Some(NOT_CONFIGURED.to_string()), None, false);
            return Ok(());
        };
        let (value, expires_at_ms, error, install_url) = match tokens.mint(repo).await {
            Ok(token) => (token.token, Some(token.expires_at_ms), None, None),
            Err(err @ GithubError::NotInstalled { .. }) => {
                let install_url = match &err {
                    GithubError::NotInstalled { install_url, .. } => install_url.clone(),
                    _ => None,
                };
                (
                    zeroize::Zeroizing::new(UNLINKED.to_string()),
                    None,
                    Some(err.to_string()),
                    install_url,
                )
            }
            Err(err) => {
                // Keep whatever the secret holds; a token that is still
                // valid there keeps working.
                let known = self
                    .repos
                    .lock()
                    .get(repo)
                    .is_some_and(|s| s.hash.is_some());
                self.fail(repo, err.to_string(), None);
                if known {
                    return Ok(());
                }
                (
                    zeroize::Zeroizing::new(UNLINKED.to_string()),
                    None,
                    Some(err.to_string()),
                    None,
                )
            }
        };
        if let Err(err) = self.set(repo, &value, expires_at_ms).await {
            self.fail(repo, err.to_string(), install_url);
            if strict {
                return Err(err);
            }
            return Ok(());
        }
        let linked = error.is_none();
        self.note(repo, error, install_url, linked);
        Ok(())
    }

    /// Record how the last sync of `repo` went. A sync that gave no token
    /// backs off like a failure, so an uninstalled App is not asked
    /// about every tick.
    fn note(
        &self,
        repo: &RepoRef,
        error: Option<String>,
        install_url: Option<String>,
        linked: bool,
    ) {
        let mut repos = self.repos.lock();
        let state = repos.entry(repo.clone()).or_default();
        state.install_url = install_url;
        if linked {
            state.error = None;
            state.failures = 0;
            state.retry_at = None;
        } else {
            state.error = error;
            Self::back_off(state);
        }
    }

    fn fail(&self, repo: &RepoRef, error: String, install_url: Option<String>) {
        let mut repos = self.repos.lock();
        let state = repos.entry(repo.clone()).or_default();
        state.error = Some(error);
        if install_url.is_some() {
            state.install_url = install_url;
        }
        Self::back_off(state);
    }

    fn back_off(state: &mut RepoState) {
        state.failures = state.failures.saturating_add(1);
        let backoff = Duration::from_secs(30)
            .saturating_mul(1 << state.failures.min(6))
            .min(MAX_BACKOFF);
        state.retry_at = Some(Instant::now() + backoff);
    }

    /// Set `repo`'s secret to `value` unless it already holds it.
    async fn set(
        &self,
        repo: &RepoRef,
        value: &str,
        expires_at_ms: Option<u64>,
    ) -> CloudResult<()> {
        let digest = hash(value);
        let unchanged = self
            .repos
            .lock()
            .get(repo)
            .and_then(|s| s.hash)
            .is_some_and(|h| h == digest);
        if !unchanged {
            self.provider
                .set_secret(&HostSecret {
                    name: self.secret_name(repo),
                    value: zeroize::Zeroizing::new(value.to_string()),
                    domains: REPO_HOSTS.iter().map(|host| (*host).to_string()).collect(),
                    replace_in: vec!["headers".to_string()],
                })
                .await?;
            tracing::info!(%repo, linked = value != UNLINKED, "repo secret set");
        }
        let mut repos = self.repos.lock();
        let state = repos.entry(repo.clone()).or_default();
        if !unchanged {
            state.hash = Some(digest);
            state.synced_ms = Some(now_ms());
        }
        state.expires_at_ms = expires_at_ms;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeProvider;
    use crate::github::FakeRepoTokens;

    fn repo(name: &str) -> RepoRef {
        RepoRef {
            owner: "acme".into(),
            name: name.into(),
        }
    }

    fn sync() -> (RepoSync, FakeProvider, Arc<FakeRepoTokens>) {
        let fake = FakeProvider::new();
        let tokens = Arc::new(FakeRepoTokens::new("aura-agents"));
        let sync = RepoSync::new(Arc::new(fake.clone()), Some(tokens.clone()), "inst01abcdef");
        (sync, fake, tokens)
    }

    fn in_minutes(minutes: u64) -> u64 {
        now_ms() + minutes * 60 * 1000
    }

    #[tokio::test]
    async fn a_repo_gets_one_secret_bound_to_github_headers() {
        let (sync, fake, tokens) = sync();
        tokens.install(&repo("app"), "ghs_one", in_minutes(60));
        let status = sync.ensure(&repo("app")).await.expect("ensure");
        assert!(status.linked);
        assert_eq!(status.error, None);
        assert!(names::is_repo_secret(&status.secret));
        assert!(
            status.secret.starts_with("AURA_GH_INST01_"),
            "{}",
            status.secret
        );
        assert_eq!(
            fake.secret_value(&status.secret).as_deref(),
            Some("ghs_one")
        );
        let secret = &fake.secrets()[&status.secret];
        assert_eq!(secret.domains, ["github.com", "api.github.com"]);
        assert_eq!(secret.replace_in, ["headers"]);
        assert!(!format!("{sync:?}{status:?}").contains("ghs_"));
    }

    #[tokio::test]
    async fn tokens_are_reminted_only_inside_the_margin() {
        let (sync, fake, tokens) = sync();
        tokens.install(&repo("app"), "ghs_one", in_minutes(60));
        let name = sync.ensure(&repo("app")).await.expect("ensure").secret;
        sync.tick(&[repo("app")]).await;
        assert_eq!(tokens.mints(&repo("app")), 1, "an hour left: no mint");

        // Ten minutes left: inside the 15-minute margin.
        sync.repos
            .lock()
            .get_mut(&repo("app"))
            .expect("state")
            .expires_at_ms = Some(in_minutes(10));
        tokens.install(&repo("app"), "ghs_two", in_minutes(60));
        sync.tick(&[repo("app")]).await;
        assert_eq!(tokens.mints(&repo("app")), 2);
        assert_eq!(fake.secret_value(&name).as_deref(), Some("ghs_two"));
        assert_eq!(
            fake.calls_matching("SetSecret(").len(),
            2,
            "rotation keeps the name"
        );
    }

    #[tokio::test]
    async fn a_repo_without_the_app_holds_unlinked_with_the_install_link() {
        let (sync, fake, tokens) = sync();
        let status = sync.ensure(&repo("private")).await.expect("ensure");
        assert!(!status.linked);
        assert_eq!(fake.secret_value(&status.secret).as_deref(), Some(UNLINKED));
        assert_eq!(
            status.install_url.as_deref(),
            Some("https://github.com/apps/aura-agents/installations/new")
        );
        assert!(status.error.is_some_and(|e| e.contains("isn't installed")));

        // Backing off: the next tick doesn't ask GitHub again.
        sync.tick(&[repo("private")]).await;
        assert_eq!(tokens.mints(&repo("private")), 1);

        // Installed later: a placement (strict) picks it up at once.
        tokens.install(&repo("private"), "ghs_now", in_minutes(60));
        let status = sync.ensure(&repo("private")).await.expect("ensure");
        assert!(status.linked);
        assert_eq!(status.install_url, None);
        assert_eq!(
            fake.secret_value(&status.secret).as_deref(),
            Some("ghs_now")
        );
    }

    #[tokio::test]
    async fn a_failing_mint_keeps_the_old_token_and_backs_off_alone() {
        let (sync, fake, tokens) = sync();
        tokens.install(&repo("a"), "ghs_a1", in_minutes(10));
        tokens.install(&repo("b"), "ghs_b1", in_minutes(10));
        let a = sync.ensure(&repo("a")).await.expect("a").secret;
        let b = sync.ensure(&repo("b")).await.expect("b").secret;
        tokens.fail(
            &repo("a"),
            GithubError::Transport {
                op: "token mint",
                message: "timed out".into(),
            },
        );
        tokens.install(&repo("b"), "ghs_b2", in_minutes(60));
        sync.tick(&[repo("a"), repo("b")]).await;
        assert_eq!(fake.secret_value(&a).as_deref(), Some("ghs_a1"), "kept");
        assert_eq!(fake.secret_value(&b).as_deref(), Some("ghs_b2"));
        assert!(sync
            .status(&repo("a"))
            .and_then(|s| s.error)
            .is_some_and(|e| e.contains("timed out")));
        sync.tick(&[repo("a")]).await;
        assert_eq!(tokens.mints(&repo("a")), 2, "backing off");
    }

    #[tokio::test]
    async fn without_an_app_every_repo_holds_unlinked() {
        let fake = FakeProvider::new();
        let sync = RepoSync::new(Arc::new(fake.clone()), None, "inst01");
        assert!(!sync.configured());
        let status = sync.ensure(&repo("app")).await.expect("ensure");
        assert!(!status.linked);
        assert!(status
            .error
            .is_some_and(|e| e.contains("AURA_GITHUB_APP_ID")));
        assert_eq!(fake.secret_value(&status.secret).as_deref(), Some(UNLINKED));
        assert_eq!(sync.identity().await, None);
    }
}
