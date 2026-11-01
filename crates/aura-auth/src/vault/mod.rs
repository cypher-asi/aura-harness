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
//! TUI observe each other's changes.

mod slot;

use crate::error::AuthError;
use aura_core_auth::{LinkedAccount, OAuthTokens, Owner, ProviderKind, StoredSession};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::warn;

pub(crate) use slot::SecretSlot;

const INDEX_VERSION: u32 = 1;

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
        })
    }

    /// File-only vault rooted at `root` (tests, sandboxes).
    #[must_use]
    pub fn files_at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            use_keyring: false,
        }
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
        let raw = self.zos_slot().load()?;
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
        self.zos_slot().save(&serde_json::to_string(session)?)
    }

    /// Remove the zOS session. Linked subscriptions are kept.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn clear_zos_session(&self) -> Result<(), AuthError> {
        self.zos_slot().clear()
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
        self.link_slot(link).save(&serde_json::to_string(tokens)?)?;
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
            let _ = self.link_slot(&old).clear();
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

    /// Tokens for `link`.
    #[must_use]
    pub fn tokens(&self, link: &LinkedAccount) -> Option<OAuthTokens> {
        let raw = self.link_slot(link).load()?;
        serde_json::from_str(&raw)
            .map_err(|e| warn!(error = %e, link = %link.id, "Stored tokens unreadable"))
            .ok()
    }

    /// Replace the tokens for `link`.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn save_tokens(&self, link: &LinkedAccount, tokens: &OAuthTokens) -> Result<(), AuthError> {
        self.link_slot(link).save(&serde_json::to_string(tokens)?)
    }

    /// Delete the tokens for `link`, keeping its metadata.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn clear_tokens(&self, link: &LinkedAccount) -> Result<(), AuthError> {
        self.link_slot(link).clear()
    }

    /// Delete `link` and its tokens.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn remove_link(&self, link: &LinkedAccount) -> Result<(), AuthError> {
        self.link_slot(link).clear()?;
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
            let _ = self.link_slot(&link).clear();
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
