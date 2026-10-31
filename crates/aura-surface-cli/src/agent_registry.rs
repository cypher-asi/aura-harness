//! Persistent list of the agents the terminal UI manages.
//!
//! Stored as `<data_dir>/agents.json`. The legacy single-agent
//! `agent_identity.txt` is folded in as the first entry so existing
//! records stay attached to the agent that wrote them.

use anyhow::Context;
use aura_core_types::{AgentId, Identity};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const REGISTRY_FILE: &str = "agents.json";
const LEGACY_IDENTITY_FILE: &str = "agent_identity.txt";
const DEFAULT_AGENT_NAME: &str = "Terminal Agent";

/// One agent known to the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredAgent {
    pub zns_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

impl RegisteredAgent {
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::new(&self.zns_id, &self.name)
    }

    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.identity().agent_id
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    /// ZNS id of the agent that was active when the TUI last switched.
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    agents: Vec<RegisteredAgent>,
}

/// File-backed agent registry.
#[derive(Debug)]
pub struct AgentRegistry {
    path: PathBuf,
    file: RegistryFile,
}

impl AgentRegistry {
    /// Load `<data_dir>/agents.json`, seeding it from the legacy identity
    /// file (or a fresh identity) when it does not exist yet.
    ///
    /// # Errors
    /// Returns an error if the registry or identity file cannot be read,
    /// parsed, or written.
    pub fn load_or_init(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join(REGISTRY_FILE);
        let file = if path.exists() {
            let body =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?
        } else {
            RegistryFile::default()
        };

        let mut registry = Self { path, file };
        if registry.file.agents.is_empty() {
            let zns_id = legacy_or_new_zns_id(data_dir)?;
            registry.file.agents.push(RegisteredAgent {
                zns_id,
                name: DEFAULT_AGENT_NAME.to_string(),
                created_at: Utc::now(),
            });
            registry.save()?;
        }
        Ok(registry)
    }

    #[must_use]
    pub fn agents(&self) -> &[RegisteredAgent] {
        &self.file.agents
    }

    #[must_use]
    pub fn find(&self, agent_id: AgentId) -> Option<&RegisteredAgent> {
        self.file.agents.iter().find(|a| a.agent_id() == agent_id)
    }

    /// The agent to open on startup: the last active one, else the first.
    #[must_use]
    pub fn active(&self) -> &RegisteredAgent {
        self.file
            .active
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns))
            .unwrap_or(&self.file.agents[0])
    }

    /// Remember `agent_id` as the active agent.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn set_active(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        if let Some(agent) = self.find(agent_id) {
            self.file.active = Some(agent.zns_id.clone());
            self.save()?;
        }
        Ok(())
    }

    /// Register a new agent. Blank names fall back to `Agent N`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create(&mut self, name: Option<String>) -> anyhow::Result<RegisteredAgent> {
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Agent {}", self.file.agents.len() + 1));
        let agent = RegisteredAgent {
            zns_id: format!("0://terminal/{}", uuid::Uuid::new_v4()),
            name,
            created_at: Utc::now(),
        };
        self.file.agents.push(agent.clone());
        self.save()?;
        Ok(agent)
    }

    fn save(&self) -> anyhow::Result<()> {
        let body =
            serde_json::to_string_pretty(&self.file).context("serialising agent registry")?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        if self.path.exists() {
            // Windows fs::rename refuses to overwrite; remove first.
            let _ = fs::remove_file(&self.path);
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

fn legacy_or_new_zns_id(data_dir: &Path) -> anyhow::Result<String> {
    let identity_file = data_dir.join(LEGACY_IDENTITY_FILE);
    if identity_file.exists() {
        let zns_id = fs::read_to_string(&identity_file)
            .with_context(|| format!("reading {}", identity_file.display()))?;
        return Ok(zns_id);
    }
    let zns_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
    fs::write(&identity_file, &zns_id)
        .with_context(|| format!("writing {}", identity_file.display()))?;
    Ok(zns_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_legacy_identity_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(LEGACY_IDENTITY_FILE), "0://terminal/legacy").unwrap();

        let registry = AgentRegistry::load_or_init(dir.path()).unwrap();

        assert_eq!(registry.agents().len(), 1);
        assert_eq!(registry.active().zns_id, "0://terminal/legacy");
        assert_eq!(registry.active().name, DEFAULT_AGENT_NAME);
        assert_eq!(
            registry.active().agent_id(),
            Identity::new("0://terminal/legacy", "x").agent_id
        );
        assert!(dir.path().join(REGISTRY_FILE).exists());
    }

    #[test]
    fn create_and_set_active_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let first = registry.active().clone();

        let named = registry.create(Some("  Researcher ".to_string())).unwrap();
        let unnamed = registry.create(None).unwrap();
        assert_eq!(named.name, "Researcher");
        assert_eq!(unnamed.name, "Agent 3");
        registry.set_active(named.agent_id()).unwrap();

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        assert_eq!(reloaded.agents(), &[first, named.clone(), unnamed]);
        assert_eq!(reloaded.active(), &named);
    }
}
