//! Provider identifiers and credential ownership.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Every auth method Aura understands.
///
/// `Local` and `Zos` are identities (who owns this machine's
/// credentials); `Chatgpt` and `Claude` are subscriptions that supply
/// inference credentials and are linked to an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// No account; local models only.
    Local,
    /// zOS account (aura-router JWT).
    Zos,
    /// Personal ChatGPT plan.
    Chatgpt,
    /// Personal Claude Pro/Max plan.
    Claude,
}

impl ProviderKind {
    /// Every subscription provider, in display order.
    pub const SUBSCRIPTIONS: [Self; 2] = [Self::Chatgpt, Self::Claude];

    /// Stable lowercase id used on the CLI and on disk.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Zos => "zos",
            Self::Chatgpt => "chatgpt",
            Self::Claude => "claude",
        }
    }

    /// Human-readable name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Zos => "zOS",
            Self::Chatgpt => "ChatGPT",
            Self::Claude => "Claude",
        }
    }

    /// Whether this provider supplies inference credentials linked to an identity.
    #[must_use]
    pub const fn is_subscription(self) -> bool {
        matches!(self, Self::Chatgpt | Self::Claude)
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Returned when parsing an unrecognised provider id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown auth provider `{0}` (expected local, zos, chatgpt or claude)")]
pub struct UnknownProvider(pub String);

impl FromStr for ProviderKind {
    type Err = UnknownProvider;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "zos" | "zero" => Ok(Self::Zos),
            "chatgpt" | "openai" => Ok(Self::Chatgpt),
            "claude" | "anthropic" => Ok(Self::Claude),
            other => Err(UnknownProvider(other.to_string())),
        }
    }
}

/// The identity that owns a subscription link.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Owner {
    /// Not signed in to any account.
    Local,
    /// A zOS user, keyed by its immutable user id.
    Zos {
        /// zOS user id.
        user_id: String,
    },
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Zos { user_id } => write!(f, "zos:{user_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_round_trip() {
        for kind in [
            ProviderKind::Local,
            ProviderKind::Zos,
            ProviderKind::Chatgpt,
            ProviderKind::Claude,
        ] {
            assert_eq!(kind.id().parse::<ProviderKind>().unwrap(), kind);
        }
        assert_eq!(
            "Anthropic".parse::<ProviderKind>().unwrap(),
            ProviderKind::Claude
        );
        assert!("github".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn owner_serialises_tagged() {
        let owner = Owner::Zos {
            user_id: "u-1".into(),
        };
        let json = serde_json::to_value(&owner).unwrap();
        assert_eq!(json["kind"], "zos");
        assert_eq!(json["user_id"], "u-1");
        assert_eq!(owner.to_string(), "zos:u-1");
    }
}
