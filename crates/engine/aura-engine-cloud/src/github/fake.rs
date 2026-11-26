//! [`FakeRepoTokens`]: repository tokens in memory, for tests.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use zeroize::Zeroizing;

use super::{BotIdentity, GithubError, RepoRef, RepoToken, RepoTokens};

/// What [`FakeRepoTokens`] mints for a repo: a token and when it expires.
type FakeMint = Result<(String, u64), GithubError>;

/// [`RepoTokens`] in memory, for tests.
#[derive(Debug)]
pub struct FakeRepoTokens {
    identity: BotIdentity,
    tokens: Mutex<HashMap<RepoRef, FakeMint>>,
    mints: Mutex<Vec<RepoRef>>,
}

impl FakeRepoTokens {
    /// An App with slug `slug`, installed nowhere yet.
    #[must_use]
    pub fn new(slug: &str) -> Self {
        Self {
            identity: BotIdentity {
                slug: slug.to_string(),
                account_id: Some(41_000_000),
            },
            tokens: Mutex::new(HashMap::new()),
            mints: Mutex::new(Vec::new()),
        }
    }

    /// From now on, minting for `repo` gives `token`, expiring at
    /// `expires_at_ms`.
    pub fn install(&self, repo: &RepoRef, token: &str, expires_at_ms: u64) {
        self.tokens
            .lock()
            .insert(repo.clone(), Ok((token.to_string(), expires_at_ms)));
    }

    /// From now on, minting for `repo` fails with `err`.
    pub fn fail(&self, repo: &RepoRef, err: GithubError) {
        self.tokens.lock().insert(repo.clone(), Err(err));
    }

    /// How often a token for `repo` was asked for.
    #[must_use]
    pub fn mints(&self, repo: &RepoRef) -> usize {
        self.mints.lock().iter().filter(|r| *r == repo).count()
    }
}

#[async_trait]
impl RepoTokens for FakeRepoTokens {
    async fn identity(&self) -> Result<BotIdentity, GithubError> {
        Ok(self.identity.clone())
    }

    async fn mint(&self, repo: &RepoRef) -> Result<RepoToken, GithubError> {
        self.mints.lock().push(repo.clone());
        match self.tokens.lock().get(repo).cloned() {
            Some(Ok((token, expires_at_ms))) => Ok(RepoToken {
                token: Zeroizing::new(token),
                expires_at_ms,
            }),
            Some(Err(err)) => Err(err),
            None => Err(GithubError::NotInstalled {
                repo: repo.full_name(),
                install_url: Some(self.identity.install_url()),
            }),
        }
    }
}
