//! Persistent auth state for every provider.
//!
//! Layout under the vault root (`~/.aura` by default):
//!
//! - zOS session: keyring `aura/credentials`, fallback `credentials.json`
//!   (unchanged from the single-provider store, so existing logins survive).
//! - `auth/index.json`: non-secret [`LinkedAccount`] metadata and issued
//!   client ids. Written atomically with mode 0600.
//! - Subscription tokens: keyring `aura/<provider>:<link id>`, fallback
//!   `auth/secrets/<link id>.json`. One entry per link keeps each secret
//!   well under platform keyring size limits.
//!
//! Every operation re-reads the index so a CLI command and a running
//! TUI observe each other's changes. Secrets are kept once read: a
//! link's tokens until the index shows another expiry for it (every
//! refresh, by any process, changes it, and a new sign-in makes a new
//! link), the zOS session for [`ZOS_SESSION_TTL`]. Reading the keyring
//! for every model request opens a Secret Service session each time,
//! and gnome-keyring-daemon aborts under that churn.

mod slot;

use crate::error::AuthError;
use aura_core_auth::{LinkedAccount, OAuthTokens, Owner, ProviderKind, StoredSession};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tracing::warn;

use slot::Read;
pub(crate) use slot::SecretSlot;

const INDEX_VERSION: u32 = 1;

/// How long a zOS session read is used before it is read again, so a
/// sign-in or sign-out by another process shows up.
const ZOS_SESSION_TTL: Duration = Duration::from_secs(30);

/// Secrets this vault read or wrote.
#[derive(Default)]
struct Kept {
    /// The raw zOS session, `None` when signed out, and when it was read
    zos: Option<(Instant, Option<String>)>,
    /// Tokens by link id, with the link's expiry in the index then
    tokens: HashMap<String, (DateTime<Utc>, OAuthTokens)>,
}

impl fmt::Debug for Kept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Kept { .. }")
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    version: u32,
    #[serde(default)]
    links: Vec<LinkedAccount>,
    /// Client ids issued to unfinished sign-ins, keyed `provider|owner`,
    /// so a retry reuses the registration instead of minting another.
    #[serde(default)]
    issued_clients: BTreeMap<String, String>,
}

/// On-disk auth state.
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
    use_keyring: bool,
    kept: Arc<Mutex<Kept>>,
}

impl Vault {
    /// Vault at `~/.aura`, keyring-first.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::NoHomeDir`] when the home directory is unknown.
    pub fn default_location() -> Result<Self, AuthError> {
        let root = dirs::home_dir()
            .map(|h| h.join(".aura"))
            .ok_or(AuthError::NoHomeDir)?;
        Ok(Self {
            root,
            use_keyring: true,
            kept: Arc::default(),
        })
    }

    /// File-only vault rooted at `root` (tests, sandboxes).
    #[must_use]
    pub fn files_at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            use_keyring: false,
            kept: Arc::default(),
        }
    }

    fn kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Vault root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn slot(&self, keyring_user: &str, path: PathBuf) -> SecretSlot {
        if self.use_keyring {
            SecretSlot::keyring(keyring_user, path)
        } else {
            SecretSlot::file(path)
        }
    }

    fn zos_slot(&self) -> SecretSlot {
        self.slot("credentials", self.root.join("credentials.json"))
    }

    fn link_slot(&self, link: &LinkedAccount) -> SecretSlot {
        self.slot(
            &format!("{}:{}", link.provider.id(), link.id),
            self.root
                .join("auth")
                .join("secrets")
                .join(format!("{}.json", link.id)),
        )
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("auth").join("index.json")
    }

    // ---- zOS session -----------------------------------------------------

    /// The stored zOS session, if any.
    #[must_use]
    pub fn zos_session(&self) -> Option<StoredSession> {
        let kept = self.kept().zos.clone();
        let raw = match kept {
            Some((read_at, raw)) if read_at.elapsed() < ZOS_SESSION_TTL => raw,
            kept => {
                let raw = match self.zos_slot().read() {
                    Read::Found(raw) => Some(raw),
                    Read::Absent => None,
                    Read::Unanswered => kept?.1,
                };
                self.kept().zos = Some((Instant::now(), raw.clone()));
                raw
            }
        }?;
        match serde_json::from_str::<StoredSession>(&raw) {
            Ok(session) => Some(session),
            Err(e) => {
                warn!(error = %e, "Stored zOS credentials have invalid format");
                None
            }
        }
    }

    /// Persist the zOS session.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn save_zos_session(&self, session: &StoredSession) -> Result<(), AuthError> {
        let raw = serde_json::to_string(session)?;
        self.zos_slot().save(&raw)?;
        self.kept().zos = Some((Instant::now(), Some(raw)));
        Ok(())
    }

    /// Remove the zOS session. Linked subscriptions are kept.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn clear_zos_session(&self) -> Result<(), AuthError> {
        self.zos_slot().clear()?;
        self.kept().zos = Some((Instant::now(), None));
        Ok(())
    }

    // ---- index -----------------------------------------------------------

    fn load_index(&self) -> Index {
        let Some(raw) = slot::load_from_path(&self.index_path()) else {
            return Index {
                version: INDEX_VERSION,
                ..Index::default()
            };
        };
        match serde_json::from_str(&raw) {
            Ok(index) => index,
            Err(e) => {
                warn!(error = %e, "Auth index is unreadable; treating as empty");
                Index {
                    version: INDEX_VERSION,
                    ..Index::default()
                }
            }
        }
    }

    fn save_index(&self, index: &Index) -> Result<(), AuthError> {
        slot::save_to_path(&self.index_path(), &serde_json::to_string_pretty(index)?)
    }

    fn update_index<T>(&self, f: impl FnOnce(&mut Index) -> T) -> Result<T, AuthError> {
        let mut index = self.load_index();
        index.version = INDEX_VERSION;
        let out = f(&mut index);
        self.save_index(&index)?;
        Ok(out)
    }

    /// Every link owned by `owner`.
    #[must_use]
    pub fn links_for(&self, owner: &Owner) -> Vec<LinkedAccount> {
        self.load_index()
            .links
            .into_iter()
            .filter(|l| &l.owner == owner)
            .collect()
    }

    /// `owner`'s link to `provider`, if any.
    #[must_use]
    pub fn link(&self, owner: &Owner, provider: ProviderKind) -> Option<LinkedAccount> {
        self.links_for(owner)
            .into_iter()
            .find(|l| l.provider == provider)
    }

    /// Insert or replace `link` (one link per owner and provider) and
    /// store its tokens. A replaced link's secret is deleted.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn put_link(&self, link: &LinkedAccount, tokens: &OAuthTokens) -> Result<(), AuthError> {
        self.save_link_tokens(link, link.expires_at, tokens)?;
        let replaced = self.update_index(|index| {
            let replaced: Vec<LinkedAccount> = index
                .links
                .iter()
                .filter(|l| l.owner == link.owner && l.provider == link.provider && l.id != link.id)
                .cloned()
                .collect();
            index
                .links
                .retain(|l| !(l.owner == link.owner && l.provider == link.provider));
            index.links.push(link.clone());
            replaced
        })?;
        for old in replaced {
            let _ = self.clear_tokens(&old);
        }
        Ok(())
    }

    /// Update non-secret metadata for an existing link.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn update_link(&self, link: &LinkedAccount) -> Result<(), AuthError> {
        self.update_index(|index| {
            if let Some(existing) = index.links.iter_mut().find(|l| l.id == link.id) {
                *existing = link.clone();
            }
        })
    }

    /// Tokens for `link`; `None` also when the keyring could not be read.
    #[must_use]
    pub fn tokens(&self, link: &LinkedAccount) -> Option<OAuthTokens> {
        self.read_tokens(link).ok().flatten()
    }

    /// Tokens for `link`, as kept since they were last read while the
    /// index still has the link's expiry from then.
    ///
    /// # Errors
    ///
    /// Fails when the keyring could not say whether it holds them.
    pub fn read_tokens(&self, link: &LinkedAccount) -> Result<Option<OAuthTokens>, AuthError> {
        if let Some((_, tokens)) = self
            .kept()
            .tokens
            .get(&link.id)
            .filter(|(expires_at, _)| *expires_at == link.expires_at)
        {
            return Ok(Some(tokens.clone()));
        }
        let tokens = match self.link_slot(link).read() {
            Read::Found(raw) => serde_json::from_str::<OAuthTokens>(&raw)
                .map_err(|e| warn!(error = %e, link = %link.id, "Stored tokens unreadable"))
                .ok(),
            Read::Absent => None,
            Read::Unanswered => {
                return Err(AuthError::CredentialKeyring(
                    "the OS keyring did not answer".into(),
                ))
            }
        };
        let mut kept = self.kept();
        match &tokens {
            Some(tokens) => {
                kept.tokens
                    .insert(link.id.clone(), (link.expires_at, tokens.clone()));
            }
            None => {
                kept.tokens.remove(&link.id);
            }
        }
        Ok(tokens)
    }

    /// Drop what is kept of `link`'s tokens, so they are read again.
    pub fn forget_tokens(&self, link: &LinkedAccount) {
        self.kept().tokens.remove(&link.id);
    }

    /// Replace the tokens for `link`, whose expiry in the index becomes
    /// theirs.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn save_tokens(&self, link: &LinkedAccount, tokens: &OAuthTokens) -> Result<(), AuthError> {
        self.save_link_tokens(link, tokens.expires_at, tokens)
    }

    fn save_link_tokens(
        &self,
        link: &LinkedAccount,
        expires_at: DateTime<Utc>,
        tokens: &OAuthTokens,
    ) -> Result<(), AuthError> {
        self.link_slot(link).save(&serde_json::to_string(tokens)?)?;
        self.kept()
            .tokens
            .insert(link.id.clone(), (expires_at, tokens.clone()));
        Ok(())
    }

    /// Delete the tokens for `link`, keeping its metadata.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn clear_tokens(&self, link: &LinkedAccount) -> Result<(), AuthError> {
        self.forget_tokens(link);
        self.link_slot(link).clear()
    }

    /// Delete `link` and its tokens.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn remove_link(&self, link: &LinkedAccount) -> Result<(), AuthError> {
        self.clear_tokens(link)?;
        self.update_index(|index| index.links.retain(|l| l.id != link.id))
    }

    /// Reassign every link owned by `from` to `to`. Links `to` already
    /// has for the same provider win; `from`'s duplicates are deleted.
    /// Returns how many links moved.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn move_links(&self, from: &Owner, to: &Owner) -> Result<usize, AuthError> {
        let (moved, dropped) = self.update_index(|index| {
            let taken: Vec<ProviderKind> = index
                .links
                .iter()
                .filter(|l| &l.owner == to)
                .map(|l| l.provider)
                .collect();
            let mut moved = 0;
            let mut dropped = Vec::new();
            for link in &mut index.links {
                if &link.owner != from {
                    continue;
                }
                if taken.contains(&link.provider) {
                    dropped.push(link.clone());
                } else {
                    link.owner = to.clone();
                    moved += 1;
                }
            }
            index
                .links
                .retain(|l| !dropped.iter().any(|d| d.id == l.id));
            (moved, dropped)
        })?;
        for link in dropped {
            let _ = self.clear_tokens(&link);
        }
        Ok(moved)
    }

    // ---- issued client ids -----------------------------------------------

    fn client_key(provider: ProviderKind, owner: &Owner) -> String {
        format!("{}|{owner}", provider.id())
    }

    /// Client id issued to an unfinished sign-in for `owner`.
    #[must_use]
    pub fn issued_client(&self, provider: ProviderKind, owner: &Owner) -> Option<String> {
        self.load_index()
            .issued_clients
            .get(&Self::client_key(provider, owner))
            .cloned()
    }

    /// Remember (or with `None`, forget) an issued client id.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn set_issued_client(
        &self,
        provider: ProviderKind,
        owner: &Owner,
        client: Option<&str>,
    ) -> Result<(), AuthError> {
        let key = Self::client_key(provider, owner);
        self.update_index(|index| match client {
            Some(c) => {
                index.issued_clients.insert(key, c.to_string());
            }
            None => {
                index.issued_clients.remove(&key);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_auth::LinkStatus;
    use chrono::Utc;

    fn link(id: &str, owner: Owner, provider: ProviderKind) -> LinkedAccount {
        LinkedAccount {
            id: id.into(),
            provider,
            owner,
            subject: "sub".into(),
            label: "a@b.c".into(),
            client_id: "client".into(),
            plan_enabled: true,
            expires_at: Utc::now(),
            created_at: Utc::now(),
            status: LinkStatus::Active,
            plan: None,
        }
    }

    fn tokens(access: &str) -> OAuthTokens {
        OAuthTokens {
            access_token: access.into(),
            refresh_token: Some("r".into()),
            id_token: None,
            scope: String::new(),
            expires_at: Utc::now(),
        }
    }

    fn zos(user: &str) -> Owner {
        Owner::Zos {
            user_id: user.into(),
        }
    }

    #[test]
    fn legacy_zos_file_is_read_in_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials.json"),
            r#"{"access_token":"jwt","user_id":"u","display_name":"n","primary_zid":"z","created_at":"2025-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let vault = Vault::files_at(dir.path());
        assert_eq!(vault.zos_session().unwrap().access_token, "jwt");
        vault.clear_zos_session().unwrap();
        assert!(vault.zos_session().is_none());
    }

    #[test]
    fn the_zos_session_is_kept_for_a_while_then_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        std::fs::write(
            &file,
            r#"{"access_token":"jwt","user_id":"u","display_name":"n","primary_zid":"z","created_at":"2025-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let vault = Vault::files_at(dir.path());
        assert!(vault.zos_session().is_some());
        std::fs::remove_file(&file).unwrap();
        assert!(vault.zos_session().is_some(), "kept, not read again");
        vault.kept().zos.as_mut().unwrap().0 -= ZOS_SESSION_TTL;
        assert!(vault.zos_session().is_none(), "read again once stale");
    }

    #[test]
    fn tokens_are_read_again_when_the_index_has_another_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::files_at(dir.path());
        let l = link("l1", Owner::Local, ProviderKind::Claude);
        vault.put_link(&l, &tokens("a1")).unwrap();
        let other_process = Vault::files_at(dir.path());
        let rotated = tokens("a2");
        other_process.save_tokens(&l, &rotated).unwrap();
        assert_eq!(vault.tokens(&l).unwrap().access_token, "a1");
        let refreshed = LinkedAccount {
            expires_at: rotated.expires_at,
            ..l
        };
        assert_eq!(vault.tokens(&refreshed).unwrap().access_token, "a2");
    }

    #[test]
    fn links_are_scoped_by_owner_and_replaced_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::files_at(dir.path());
        let first = link("l1", zos("u"), ProviderKind::Chatgpt);
        vault.put_link(&first, &tokens("a1")).unwrap();
        vault
            .put_link(
                &link("l2", Owner::Local, ProviderKind::Chatgpt),
                &tokens("a2"),
            )
            .unwrap();
        assert_eq!(vault.links_for(&zos("u")).len(), 1);
        assert_eq!(vault.links_for(&Owner::Local).len(), 1);
        assert!(vault.links_for(&zos("other")).is_empty());

        let second = link("l3", zos("u"), ProviderKind::Chatgpt);
        vault.put_link(&second, &tokens("a3")).unwrap();
        assert_eq!(
            vault.link(&zos("u"), ProviderKind::Chatgpt).unwrap().id,
            "l3"
        );
        assert!(
            vault.tokens(&first).is_none(),
            "replaced link secret removed"
        );
        assert_eq!(vault.tokens(&second).unwrap().access_token, "a3");
    }

    #[cfg(unix)]
    #[test]
    fn secrets_are_private_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::files_at(dir.path());
        let l = link("l1", Owner::Local, ProviderKind::Claude);
        vault.put_link(&l, &tokens("secret")).unwrap();
        let secret = dir.path().join("auth/secrets/l1.json");
        let mode = std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let index = std::fs::read_to_string(dir.path().join("auth/index.json")).unwrap();
        assert!(!index.contains("secret"), "index must not hold tokens");
    }

    #[test]
    fn move_links_keeps_existing_target_links() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::files_at(dir.path());
        vault
            .put_link(
                &link("local-gpt", Owner::Local, ProviderKind::Chatgpt),
                &tokens("a"),
            )
            .unwrap();
        vault
            .put_link(
                &link("local-claude", Owner::Local, ProviderKind::Claude),
                &tokens("b"),
            )
            .unwrap();
        vault
            .put_link(
                &link("zos-claude", zos("u"), ProviderKind::Claude),
                &tokens("c"),
            )
            .unwrap();
        assert_eq!(vault.move_links(&Owner::Local, &zos("u")).unwrap(), 1);
        assert!(vault.links_for(&Owner::Local).is_empty());
        let mine = vault.links_for(&zos("u"));
        assert_eq!(mine.len(), 2);
        assert_eq!(
            vault.link(&zos("u"), ProviderKind::Claude).unwrap().id,
            "zos-claude"
        );
    }

    #[test]
    fn issued_clients_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::files_at(dir.path());
        let owner = Owner::Local;
        assert!(vault.issued_client(ProviderKind::Chatgpt, &owner).is_none());
        vault
            .set_issued_client(ProviderKind::Chatgpt, &owner, Some("issued"))
            .unwrap();
        assert_eq!(
            vault
                .issued_client(ProviderKind::Chatgpt, &owner)
                .as_deref(),
            Some("issued")
        );
        vault
            .set_issued_client(ProviderKind::Chatgpt, &owner, None)
            .unwrap();
        assert!(vault.issued_client(ProviderKind::Chatgpt, &owner).is_none());
    }
}
