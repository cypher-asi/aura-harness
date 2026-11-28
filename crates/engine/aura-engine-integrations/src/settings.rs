//! Non-secret settings: each integration's config values and who may
//! use it, in one JSON file (`config.json`) next to the sealed
//! credentials. Nothing secret goes here.

use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::secret::write_private;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The settings' file name, in the integrations folder.
pub const SETTINGS_FILE: &str = "config.json";

/// Which agents may use an integration's tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Access {
    /// Every agent.
    #[default]
    AllAgents,
    /// Only these agents (hex ids) and the members of these channels;
    /// both empty means none.
    Only {
        /// Agent ids
        #[serde(default)]
        agents: Vec<String>,
        /// Channel ids
        #[serde(default)]
        channels: Vec<String>,
    },
}

impl Access {
    /// Whether `agent`, a member of `channel` if any, may use it.
    #[must_use]
    pub fn allows(&self, agent: &str, channel: Option<&str>) -> bool {
        match self {
            Self::AllAgents => true,
            Self::Only { agents, channels } => {
                agents.iter().any(|allowed| allowed == agent)
                    || channel.is_some_and(|channel| channels.iter().any(|c| c == channel))
            }
        }
    }

    /// A few words for the panel.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::AllAgents => "all agents".to_string(),
            Self::Only { agents, channels } if agents.is_empty() && channels.is_empty() => {
                "no agents".to_string()
            }
            Self::Only { agents, channels } => {
                format!("{} agents, {} channels", agents.len(), channels.len())
            }
        }
    }
}

/// One integration's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    #[serde(default)]
    pub(crate) config: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) access: Access,
}

/// Every integration's settings, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    #[serde(default)]
    pub(crate) integrations: BTreeMap<String, Entry>,
}

impl Settings {
    pub(crate) fn entry(&self, id: &IntegrationId) -> Entry {
        self.integrations
            .get(id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn entry_mut(&mut self, id: &IntegrationId) -> &mut Entry {
        self.integrations.entry(id.to_string()).or_default()
    }

    /// The settings in `path`; none when it doesn't exist.
    pub(crate) fn load(path: &Path) -> Result<Self, IntegrationError> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| IntegrationError::Store(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(IntegrationError::Store(format!("{}: {e}", path.display()))),
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), IntegrationError> {
        let json =
            serde_json::to_vec_pretty(self).map_err(|e| IntegrationError::Store(e.to_string()))?;
        write_private(path, &json)
            .map_err(|e| IntegrationError::Store(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_lets_listed_agents_and_channels_in() {
        assert!(Access::AllAgents.allows("a", None));
        let only = Access::Only {
            agents: vec!["a".to_string()],
            channels: vec!["c".to_string()],
        };
        assert!(only.allows("a", None));
        assert!(only.allows("b", Some("c")));
        assert!(!only.allows("b", Some("d")));
        assert!(!only.allows("b", None));
        let none = Access::Only {
            agents: Vec::new(),
            channels: Vec::new(),
        };
        assert_eq!(none.describe(), "no agents");
    }
}
