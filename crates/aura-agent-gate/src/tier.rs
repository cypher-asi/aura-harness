//! Model tiers and gate model ids.
//!
//! A gate id is `gate/` followed by the frontier model it coordinates
//! with (`gate/claude/claude-opus-5-5`). The cheaper tiers come from the
//! same route as the frontier model, so routing never changes who is
//! billed.

use serde::{Deserialize, Serialize};

pub use aura_model_reasoner::model_catalog::GATE_PREFIX;

/// Capability tier of a model, cheapest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Small, fast model (Haiku class)
    Fast,
    /// Mid-size model (Sonnet class)
    Standard,
    /// The coordinator's own model (Opus class)
    Frontier,
}

impl Tier {
    /// Every tier, cheapest first.
    pub const ALL: [Self; 3] = [Self::Fast, Self::Standard, Self::Frontier];

    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Standard => "standard",
            Self::Frontier => "frontier",
        }
    }

    /// Parse [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s.trim())
    }

    /// The next stronger tier.
    #[must_use]
    pub const fn up(self) -> Option<Self> {
        match self {
            Self::Fast => Some(Self::Standard),
            Self::Standard => Some(Self::Frontier),
            Self::Frontier => None,
        }
    }

    /// The next cheaper tier.
    #[must_use]
    pub const fn down(self) -> Option<Self> {
        match self {
            Self::Fast => None,
            Self::Standard => Some(Self::Fast),
            Self::Frontier => Some(Self::Standard),
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Frontier model a gate id coordinates with
/// (`gate/claude/claude-opus-5-5` -> `claude/claude-opus-5-5`).
#[must_use]
pub fn frontier_of(model: &str) -> Option<&str> {
    aura_model_reasoner::model_catalog::gate_frontier(model)
}

/// Gate id coordinating with `frontier`.
#[must_use]
pub fn gate_id(frontier: &str) -> String {
    format!("{GATE_PREFIX}{frontier}")
}

/// Route prefixes model ids carry, longest first.
const ROUTES: &[&str] = &["claude/", "chatgpt/", "local/", "aura-"];

/// One model per tier, all on one route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tiers {
    /// Fast tier model id
    pub fast: String,
    /// Standard tier model id
    pub standard: String,
    /// Frontier tier model id
    pub frontier: String,
}

impl Tiers {
    /// Model id of `tier`.
    #[must_use]
    pub fn model(&self, tier: Tier) -> &str {
        match tier {
            Tier::Fast => &self.fast,
            Tier::Standard => &self.standard,
            Tier::Frontier => &self.frontier,
        }
    }

    /// Tiers below `frontier`: the newest smaller models of the same
    /// vendor generation on the same route. Unknown vendors and local
    /// models get every tier set to `frontier`.
    #[must_use]
    pub fn for_frontier(frontier: &str) -> Self {
        let route = ROUTES
            .iter()
            .find(|r| frontier.starts_with(**r))
            .copied()
            .unwrap_or("");
        let name = frontier[route.len()..].to_ascii_lowercase();
        let flat = || Self {
            fast: frontier.to_string(),
            standard: frontier.to_string(),
            frontier: frontier.to_string(),
        };
        let (fast, standard) = if let Some(rest) = name.strip_prefix("claude-") {
            let generation = rest
                .split('-')
                .find(|part| part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty());
            match generation {
                Some("5") => ("claude-haiku-5-5", "claude-sonnet-5-5"),
                Some("4") => ("claude-haiku-4-5", "claude-sonnet-4-6"),
                _ => return flat(),
            }
        } else if name.starts_with("gpt-5") {
            ("gpt-5.6-luna", "gpt-5.6-terra")
        } else if name.starts_with("gpt-6") {
            ("gpt-6-luna", "gpt-6-1-terra")
        } else {
            return flat();
        };
        if route == "local/" {
            return flat();
        }
        let id = |n: &str| format!("{route}{n}");
        if name.contains("haiku") || name.contains("luna") {
            flat()
        } else if name.contains("sonnet") || name.contains("terra") {
            Self {
                fast: id(fast),
                standard: frontier.to_string(),
                frontier: frontier.to_string(),
            }
        } else {
            Self {
                fast: id(fast),
                standard: id(standard),
                frontier: frontier.to_string(),
            }
        }
    }

    /// Apply `fast=<id>,standard=<id>,frontier=<id>` overrides.
    ///
    /// # Errors
    /// Fails on an unknown tier name or a missing `=`.
    pub fn with_overrides(mut self, spec: &str) -> Result<Self, String> {
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (tier, model) = part
                .split_once('=')
                .ok_or_else(|| format!("tier override `{part}` is not tier=model"))?;
            let model = model.trim().to_string();
            match Tier::parse(tier) {
                Some(Tier::Fast) => self.fast = model,
                Some(Tier::Standard) => self.standard = model,
                Some(Tier::Frontier) => self.frontier = model,
                None => return Err(format!("unknown tier `{tier}`")),
            }
        }
        Ok(self)
    }

    /// The next tier above `tier` that runs a different model.
    #[must_use]
    pub fn stronger(&self, tier: Tier) -> Option<Tier> {
        let mut next = tier.up();
        while let Some(t) = next {
            if self.model(t) != self.model(tier) {
                return Some(t);
            }
            next = t.up();
        }
        None
    }

    /// The next tier below `tier` that runs a different model.
    #[must_use]
    pub fn cheaper(&self, tier: Tier) -> Option<Tier> {
        let mut next = tier.down();
        while let Some(t) = next {
            if self.model(t) != self.model(tier) {
                return Some(t);
            }
            next = t.down();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_ids_round_trip() {
        assert_eq!(
            frontier_of("gate/claude/claude-opus-5-5"),
            Some("claude/claude-opus-5-5")
        );
        assert_eq!(frontier_of("gate/"), None);
        assert_eq!(frontier_of("claude/claude-opus-5-5"), None);
        assert_eq!(gate_id("aura-claude-opus-4-7"), "gate/aura-claude-opus-4-7");
    }

    #[test]
    fn tiers_stay_on_the_frontier_route() {
        let t = Tiers::for_frontier("claude/claude-opus-5-5");
        assert_eq!(t.fast, "claude/claude-haiku-5-5");
        assert_eq!(t.standard, "claude/claude-sonnet-5-5");
        assert_eq!(t.frontier, "claude/claude-opus-5-5");

        let t = Tiers::for_frontier("aura-claude-opus-4-7");
        assert_eq!(t.fast, "aura-claude-haiku-4-5");
        assert_eq!(t.standard, "aura-claude-sonnet-4-6");

        let t = Tiers::for_frontier("claude-sonnet-5-5");
        assert_eq!(t.fast, "claude-haiku-5-5");
        assert_eq!(t.standard, "claude-sonnet-5-5");
        assert_eq!(t.frontier, "claude-sonnet-5-5");
    }

    #[test]
    fn unknown_and_local_models_are_flat() {
        for model in ["local/qwen3:4b", "mystery-model", "claude-haiku-5-5"] {
            let t = Tiers::for_frontier(model);
            assert_eq!(t.fast, model);
            assert_eq!(t.standard, model);
            assert_eq!(t.stronger(Tier::Fast), None);
        }
    }

    #[test]
    fn overrides_and_distinct_steps() {
        let t = Tiers::for_frontier("claude/claude-opus-5-5")
            .with_overrides("standard=claude/claude-opus-5-5")
            .unwrap();
        assert_eq!(t.stronger(Tier::Fast), Some(Tier::Standard));
        assert_eq!(t.stronger(Tier::Standard), None);
        assert_eq!(t.cheaper(Tier::Frontier), Some(Tier::Fast));
        assert!(Tiers::for_frontier("x").with_overrides("huge=y").is_err());
    }
}
