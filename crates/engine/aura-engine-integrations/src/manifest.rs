//! What an integration says about itself: the [`Manifest`].

use crate::id::IntegrationId;
use std::time::Duration;

/// Everything about an integration that doesn't need a call to know.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Stable id
    pub id: IntegrationId,
    /// Name as the panel shows it (`GitHub`)
    pub name: String,
    /// One glyph, one or two columns wide, drawn before the name
    pub icon: String,
    /// One line on what it is
    pub description: String,
    /// Short phrases on what it gives aura (`pull requests`, `cloud
    /// machines`), listed in the panel
    pub provides: Vec<String>,
    /// How it signs in
    pub auth: AuthKind,
    /// Settings the user can change
    pub config: Vec<ConfigField>,
    /// Tools it gives agents
    pub tools: Vec<ToolSpec>,
    /// Whether its credential may reach cloud machines
    pub cloud: CloudDelivery,
}

impl Manifest {
    /// A manifest with only an id and a name: no sign-in, config or
    /// tools, never sent to cloud machines. Set the rest on it.
    #[must_use]
    pub fn new(id: IntegrationId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            icon: "◆".to_string(),
            description: String::new(),
            provides: Vec::new(),
            auth: AuthKind::None,
            config: Vec::new(),
            tools: Vec::new(),
            cloud: CloudDelivery::Never,
        }
    }

    /// The tool called `name` (its local name, without the prefix).
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// The config field `key`.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&ConfigField> {
        self.config.iter().find(|field| field.key == key)
    }
}

/// How an integration signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKind {
    /// Nothing to sign in to.
    None,
    /// A key or token the user pastes, also read from env variables.
    ApiKey(KeyAuth),
    /// A CLI the user signs in with (`gh auth login`), optionally with a
    /// token as another way in.
    CliLogin {
        /// The program (`gh`)
        program: String,
        /// The command that signs in (`gh auth login`)
        login: String,
        /// A token that works instead, if any
        token: Option<KeyAuth>,
    },
    /// An OAuth sign-in. Declared for the panel; the flow itself is not
    /// built yet (`docs/integrations.md`).
    OAuth {
        /// Where the user authorizes
        authorize_url: String,
        /// Scopes asked for
        scopes: Vec<String>,
    },
}

impl AuthKind {
    /// The key or token it takes, if any: what Connect asks for.
    #[must_use]
    pub const fn key(&self) -> Option<&KeyAuth> {
        match self {
            Self::ApiKey(key) => Some(key),
            Self::CliLogin { token, .. } => token.as_ref(),
            Self::None | Self::OAuth { .. } => None,
        }
    }

    /// One word for the panel: `API key`, `CLI login`, `OAuth` or `none`.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ApiKey(_) => "API key",
            Self::CliLogin { .. } => "CLI login",
            Self::OAuth { .. } => "OAuth",
        }
    }
}

/// A key or token an integration takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAuth {
    /// What it is called (`API key`, `Personal access token`)
    pub label: String,
    /// Environment variables it is also read from, first one set wins;
    /// a stored key wins over them
    pub env: Vec<String>,
    /// Where to get one
    pub hint: String,
}

/// A setting of an integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigField {
    /// Key it is stored under
    pub key: String,
    /// Label in the panel
    pub label: String,
    /// What values it takes
    pub kind: FieldKind,
    /// Whether the integration needs it set
    pub required: bool,
    /// Value when unset
    pub default: Option<String>,
}

/// The values a config field takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// Any text
    Text,
    /// An `http://` or `https://` URL
    Url,
    /// `true` or `false`
    Bool,
    /// One of these
    Choice(Vec<String>),
}

impl FieldKind {
    /// Why `value` doesn't fit, if it doesn't.
    #[must_use]
    pub fn reject(&self, value: &str) -> Option<String> {
        match self {
            Self::Text => None,
            Self::Url => (!(value.starts_with("https://") || value.starts_with("http://")))
                .then(|| "must be an http:// or https:// URL".to_string()),
            Self::Bool => {
                (!matches!(value, "true" | "false")).then(|| "must be true or false".to_string())
            }
            Self::Choice(choices) => (!choices.iter().any(|choice| choice == value))
                .then(|| format!("must be one of {}", choices.join(", "))),
        }
    }
}

/// A tool an integration gives agents.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// Local name (`list_pods`); agents see `<id>_<name>`
    /// ([`crate::tool_name`])
    pub name: String,
    /// What it does, for the model
    pub description: String,
    /// JSON schema of its arguments
    pub input_schema: serde_json::Value,
    /// Whether it only reads or also changes something
    pub effect: ToolEffect,
    /// Its own time limit, when longer or shorter than the registry's
    pub timeout: Option<Duration>,
}

/// What a tool does to the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    /// Only reads
    Read,
    /// Creates, changes or deletes something
    Write,
}

/// Whether an integration's credential may reach cloud machines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CloudDelivery {
    /// Never: it stays on the hub.
    #[default]
    Never,
    /// As a boxd host-bound secret: the machine's `env` holds only boxd's
    /// placeholder, which boxd swaps for the real value on the way out.
    HostBound {
        /// The variable machines see
        env: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_reject_values_that_dont_fit() {
        assert!(FieldKind::Text.reject("anything").is_none());
        assert!(FieldKind::Url.reject("https://x").is_none());
        assert!(FieldKind::Url.reject("x.com").is_some());
        assert!(FieldKind::Bool.reject("yes").is_some());
        let choice = FieldKind::Choice(vec!["a".to_string(), "b".to_string()]);
        assert!(choice.reject("b").is_none());
        assert_eq!(choice.reject("c").unwrap(), "must be one of a, b");
    }

    #[test]
    fn only_keys_and_tokens_are_asked_for_on_connect() {
        let key = KeyAuth {
            label: "Token".to_string(),
            env: vec!["T".to_string()],
            hint: String::new(),
        };
        assert!(AuthKind::ApiKey(key.clone()).key().is_some());
        let cli = AuthKind::CliLogin {
            program: "gh".to_string(),
            login: "gh auth login".to_string(),
            token: Some(key),
        };
        assert_eq!(cli.key().unwrap().label, "Token");
        assert!(AuthKind::None.key().is_none());
        assert_eq!(cli.label(), "CLI login");
    }
}
