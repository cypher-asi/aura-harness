//! The environment a cloud machine gets: an explicit allowlist, never a
//! copy of this process's environment.
//!
//! ## Invariants
//!
//! - No `BOXD_*` variable (the key or a token) is ever put in a
//!   machine's env; [`MachineEnv::insert`] refuses it, and anything
//!   whose name says it is a secret, unless the name is on
//!   [`ALLOWED_SECRET_NAMES`] (none today: node tokens travel over
//!   `Exec`).

use std::collections::BTreeMap;

use crate::error::{CloudError, CloudResult};

/// Port the aura node listens on in a machine; boxd's HTTPS domain
/// forwards to it.
pub const NODE_PORT: u16 = 8080;

/// Directory the project lives in on a machine.
pub const WORKSPACE_DIR: &str = "/workspace";

/// Directory the node keeps its RocksDB and state in on a machine.
pub const DATA_DIR: &str = "/var/lib/aura";

/// Secret-looking names that may still be set. Empty on purpose.
pub const ALLOWED_SECRET_NAMES: &[&str] = &[];

const SECRET_MARKERS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "JWT", "CREDENTIAL"];
const DENIED_PREFIXES: &[&str] = &["BOXD_", "DAYTONA_", "AWS_", "GOOGLE_", "AZURE_"];

/// Whether `name` may go into a machine's env.
#[must_use]
pub fn is_allowed(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if DENIED_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
    {
        return false;
    }
    if ALLOWED_SECRET_NAMES.contains(&name) {
        return true;
    }
    !SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

/// A machine's env, built entry by entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineEnv(BTreeMap<String, String>);

impl MachineEnv {
    /// The env of an agent machine with `vcpu` vCPUs.
    #[must_use]
    pub fn for_agent(vcpu: u32) -> Self {
        let mut env = Self::default();
        let entries = [
            ("AURA_LISTEN_ADDR", format!("0.0.0.0:{NODE_PORT}")),
            ("AURA_DATA_DIR", DATA_DIR.to_string()),
            ("AURA_WORKSPACE", WORKSPACE_DIR.to_string()),
            ("AURA_NODE_REQUIRE_AUTH", "1".to_string()),
            ("AURA_CLOUD_NODE", "1".to_string()),
            ("CARGO_BUILD_JOBS", vcpu.max(1).to_string()),
            ("CARGO_INCREMENTAL", "1".to_string()),
            // mold through env, not the repo's cargo config.
            ("RUSTFLAGS", "-C link-arg=-fuse-ld=mold".to_string()),
            ("RUST_LOG", "info".to_string()),
        ];
        for (name, value) in entries {
            // Every entry above is allowlisted by construction.
            env.0.insert(name.to_string(), value);
        }
        env
    }

    /// Add `name=value`.
    ///
    /// # Errors
    ///
    /// When `name` is not allowed in a machine ([`is_allowed`]).
    pub fn insert(&mut self, name: &str, value: impl Into<String>) -> CloudResult<()> {
        if !is_allowed(name) {
            return Err(CloudError::BadState {
                machine: "env".into(),
                state: format!("refusing {name}"),
                op: "pass a secret-looking variable to a machine",
            });
        }
        self.0.insert(name.to_string(), value.into());
        Ok(())
    }

    /// The entries.
    #[must_use]
    pub fn into_map(self) -> BTreeMap<String, String> {
        self.0
    }

    /// The entries, borrowed.
    #[must_use]
    pub const fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// `export` lines for a shell profile, values single-quoted.
    #[must_use]
    pub fn as_exports(&self) -> String {
        self.0
            .iter()
            .map(|(name, value)| format!("export {name}='{}'\n", value.replace('\'', r"'\''")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_env_has_no_boxd_variables() {
        let env = MachineEnv::for_agent(4);
        assert!(env.as_map().keys().all(|k| !k.starts_with("BOXD_")));
        assert!(env.as_map().keys().all(|k| is_allowed(k)));
        assert_eq!(
            env.as_map().get("CARGO_BUILD_JOBS").map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn secrets_are_refused() {
        let mut env = MachineEnv::default();
        assert!(env.insert("BOXD_API_KEY", "bxd_x").is_err());
        assert!(env.insert("BOXD_REGION", "nyc").is_err());
        assert!(env.insert("OPENAI_API_KEY", "sk").is_err());
        assert!(env.insert("AURA_ROUTER_JWT", "x").is_err());
        assert!(env.insert("AURA_LOCAL_MODEL", "qwen").is_ok());
    }

    #[test]
    fn exports_quote_values() {
        let mut env = MachineEnv::default();
        env.insert("A", "it's").expect("allowed");
        assert_eq!(env.as_exports(), "export A='it'\\''s'\n");
    }
}
