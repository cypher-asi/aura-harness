//! The integrations the Integrations panel lists (see
//! `docs/integrations.md`).

use serde::{Deserialize, Serialize};

/// Whether an integration is ready.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationState {
    /// Signed in and ready
    Connected,
    /// Not signed in
    #[default]
    NotConnected,
    /// Signed in but can't work as set, or its last action failed
    Failed,
    /// An action is running (Connect or Test)
    Busy,
}

/// One setting of an integration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationFieldView {
    /// Key it is stored under
    pub key: String,
    /// Label
    pub label: String,
    /// Value set, or its default
    #[serde(default)]
    pub value: Option<String>,
}

/// One integration as the panel shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationView {
    /// Stable id (`github`)
    pub id: String,
    /// Name
    pub name: String,
    /// One glyph
    pub icon: String,
    /// One line on what it is
    pub description: String,
    /// What it gives aura
    #[serde(default)]
    pub provides: Vec<String>,
    /// Whether it is ready
    pub state: IntegrationState,
    /// How it is signed in, how to sign in, or what is wrong
    pub status: String,
    /// What Connect asks for (`Token`, `API key`); `None` when it takes
    /// no key
    #[serde(default)]
    pub takes_key: Option<String>,
    /// The command that signs in, for a CLI login (`gh auth login`)
    #[serde(default)]
    pub login: Option<String>,
    /// Whether a key is stored, so Disconnect has something to remove
    #[serde(default)]
    pub stored: bool,
    /// Its settings
    #[serde(default)]
    pub config: Vec<IntegrationFieldView>,
    /// Its agent tools' names
    #[serde(default)]
    pub tools: Vec<String>,
    /// Who may use it
    #[serde(default)]
    pub access: String,
    /// What its last action said (`signed in as octocat`)
    #[serde(default)]
    pub note: Option<String>,
}

/// What the user asked of an integration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum IntegrationAction {
    /// Send the list again
    Refresh,
    /// Check the key and store it
    Connect {
        /// The key; never shown in `Debug`
        secret: String,
    },
    /// Set a setting
    Configure {
        /// Its key
        key: String,
        /// Its value; empty clears it
        value: String,
    },
    /// Run the health check
    Test,
    /// Remove the stored key
    Disconnect,
}

impl std::fmt::Debug for IntegrationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refresh => f.write_str("Refresh"),
            Self::Connect { .. } => f.write_str("Connect { secret: … }"),
            Self::Configure { key, value } => f
                .debug_struct("Configure")
                .field("key", key)
                .field("value", value)
                .finish(),
            Self::Test => f.write_str("Test"),
            Self::Disconnect => f.write_str("Disconnect"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connect_never_shows_its_key() {
        let action = IntegrationAction::Connect {
            secret: "ghp_secret".to_string(),
        };
        assert!(!format!("{action:?}").contains("ghp_secret"));
    }
}
