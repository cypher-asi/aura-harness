//! GitHub App tokens for cloud machines.
//!
//! The hub holds a GitHub App's private key ([`aura_config::GithubAppConfig`]).
//! For each repository cloud agents work on, [`GithubApp`] signs a
//! short-lived App JWT, looks up the App's installation on that
//! repository and mints an installation token scoped to that one
//! repository, with `contents` and `pull_requests` write (and
//! `workflows` write only when configured). [`crate::repos::RepoSync`]
//! keeps the token in a boxd host-bound secret; machines hold only its
//! placeholder. Commits, pushes and pull requests are then the App's bot,
//! `<slug>[bot]` ([`BotIdentity`]).
//!
//! ## Invariants
//!
//! - The private key and App JWTs never leave this process and are never
//!   logged; tokens are [`Zeroizing`] and [`RepoToken`]'s `Debug` hides
//!   them. Error messages carry GitHub's `message` field only, never a
//!   response body.
//! - A token is scoped to one repository and to [`GithubApp::permissions`].
//! - Every call has a timeout ([`CALL_TIMEOUT`]).

mod app;
mod fake;

pub use self::app::GithubApp;
pub use self::fake::FakeRepoTokens;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// GitHub's REST API.
pub const API_BASE: &str = "https://api.github.com";
/// Limit on one call to GitHub.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Limit on connecting to GitHub.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// App JWTs are issued this far in the past, against clock drift.
const JWT_BACKDATE_SECS: i64 = 60;
/// And expire this far ahead (GitHub allows at most ten minutes).
const JWT_LIFETIME_SECS: i64 = 9 * 60;
/// Assumed lifetime of a token whose expiry GitHub didn't state.
const ASSUMED_TOKEN_LIFETIME_MS: u64 = 50 * 60 * 1000;
/// The REST API version every call asks for.
const API_VERSION: &str = "2022-11-28";

/// A repository on github.com: `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRef {
    /// The user or organization.
    pub owner: String,
    /// The repository, without `.git`.
    pub name: String,
}

/// Whether `segment` can be a GitHub owner or repository name.
fn is_name(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl RepoRef {
    /// The repository a git remote URL names, when it is on github.com:
    /// `https://github.com/o/r(.git)`, `ssh://git@github.com[:22]/o/r.git`
    /// or `git@github.com:o/r.git`. User-info is ignored. `None` for
    /// other hosts and anything else.
    #[must_use]
    pub fn from_remote(url: &str) -> Option<Self> {
        let url = url.trim();
        let (host, path) = match url.split_once("://") {
            Some((scheme, rest)) => {
                if !matches!(scheme, "https" | "http" | "ssh" | "git") {
                    return None;
                }
                let (authority, path) = rest.split_once('/')?;
                let host = authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host);
                (host.split(':').next()?, path)
            }
            None => {
                // scp-like `[user@]host:path`.
                let (authority, path) = url.split_once(':')?;
                if authority.contains('/') {
                    return None;
                }
                (
                    authority
                        .rsplit_once('@')
                        .map_or(authority, |(_, host)| host),
                    path,
                )
            }
        };
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        let path = path.trim_start_matches('/').trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        (is_name(owner) && is_name(name)).then(|| Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// `owner/name`.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The https remote machines push to.
    #[must_use]
    pub fn https_url(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The App's bot, who commits, pushes and opens pull requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotIdentity {
    /// The App's slug (`aura-agents`).
    pub slug: String,
    /// The bot account's GitHub id, which makes its commit email link
    /// to it; `None` when it couldn't be looked up.
    pub account_id: Option<u64>,
}

impl BotIdentity {
    /// `<slug>[bot]`, the git author name.
    #[must_use]
    pub fn login(&self) -> String {
        format!("{}[bot]", self.slug)
    }

    /// `<id>+<slug>[bot]@users.noreply.github.com`, the git author email.
    #[must_use]
    pub fn email(&self) -> String {
        match self.account_id {
            Some(id) => format!("{id}+{}[bot]@users.noreply.github.com", self.slug),
            None => format!("{}[bot]@users.noreply.github.com", self.slug),
        }
    }

    /// Where a repository owner installs the App.
    #[must_use]
    pub fn install_url(&self) -> String {
        format!("https://github.com/apps/{}/installations/new", self.slug)
    }
}

/// An installation token for one repository.
pub struct RepoToken {
    /// The token (`ghs_…`); never logged.
    pub token: Zeroizing<String>,
    /// When GitHub stops accepting it, Unix milliseconds.
    pub expires_at_ms: u64,
}

impl std::fmt::Debug for RepoToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoToken")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// Why a repository token could not be had.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubError {
    /// The App isn't installed on the repository, or the installation
    /// doesn't include it.
    #[error(
        "the GitHub App isn't installed on {repo}{}",
        install_url.as_ref().map(|url| format!(": install it at {url}")).unwrap_or_default()
    )]
    NotInstalled {
        /// `owner/name`.
        repo: String,
        /// Where to install it, when the App's slug is known.
        install_url: Option<String>,
    },
    /// The private key could not be read or used.
    #[error("GitHub App key {path}: {message}")]
    Key {
        /// The key file.
        path: String,
        /// What was wrong (never the key).
        message: String,
    },
    /// GitHub answered with an error.
    #[error("GitHub {op} failed ({status}): {message}")]
    Api {
        /// The call.
        op: &'static str,
        /// HTTP status.
        status: u16,
        /// GitHub's `message`.
        message: String,
    },
    /// GitHub could not be reached in time.
    #[error("could not reach GitHub for {op}: {message}")]
    Transport {
        /// The call.
        op: &'static str,
        /// Why.
        message: String,
    },
    /// GitHub answered with something this build can't use.
    #[error("unexpected answer from GitHub {op}: {message}")]
    InvalidResponse {
        /// The call.
        op: &'static str,
        /// What was wrong.
        message: String,
    },
}

/// Where the hub gets repository tokens. [`GithubApp`] implements it over
/// GitHub; [`FakeRepoTokens`] in memory.
#[async_trait]
pub trait RepoTokens: Send + Sync {
    /// The bot commits and pull requests are made as.
    ///
    /// # Errors
    ///
    /// When GitHub can't be asked or the key can't sign.
    async fn identity(&self) -> Result<BotIdentity, GithubError>;

    /// A token for `repo` alone.
    ///
    /// # Errors
    ///
    /// [`GithubError::NotInstalled`] when the App can't reach `repo`;
    /// otherwise as [`Self::identity`].
    async fn mint(&self, repo: &RepoRef) -> Result<RepoToken, GithubError>;
}

#[cfg(test)]
mod tests;
