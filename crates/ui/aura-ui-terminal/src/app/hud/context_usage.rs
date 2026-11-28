//! What fills an agent's context window, part by part, for the CONTEXT
//! panel and the ring in the footer.
//!
//! The total is the real one: the prompt tokens the model reported for
//! the agent's last call (`SessionStats::context_tokens`), of the model's
//! window. The parts are measured from their text by the agent loop
//! after each turn ([`ContextView`]): the system prompt, the tool, MCP,
//! subagent and skill definitions. Messages are the rest of the real
//! total, so the parts always add up to it. Before the model has reported
//! a call, the loop's own estimate stands in, and the panel says so.

use super::Hud;
use crate::events::ContextView;

/// One part of the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPart {
    /// The system prompt, project instructions included
    System,
    /// Built-in and plugin tool definitions
    Tools,
    /// MCP servers' tool definitions
    Mcp,
    /// The subagents the agent may start
    Subagents,
    /// Skills
    Skills,
    /// The conversation: prompts, replies, tool calls and results
    Messages,
    /// What is left of the window
    Free,
}

impl ContextPart {
    /// Every part, in the order the panel lists them.
    #[cfg(test)]
    pub const ALL: [Self; 7] = [
        Self::System,
        Self::Tools,
        Self::Mcp,
        Self::Subagents,
        Self::Skills,
        Self::Messages,
        Self::Free,
    ];

    /// The part's name in the panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System prompt",
            Self::Tools => "Tools",
            Self::Mcp => "MCP tools",
            Self::Subagents => "Subagents",
            Self::Skills => "Skills",
            Self::Messages => "Messages",
            Self::Free => "Free space",
        }
    }
}

/// Where the total comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSource {
    /// The prompt tokens the model reported for the last call
    Reported,
    /// The agent loop's estimate, before any call was reported
    Estimated,
}

/// An agent's context, part by part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    /// Tokens in the context now
    pub used: u64,
    /// The model's context window, when known
    pub window: Option<u64>,
    /// Each part with its tokens, in [`ContextPart::ALL`] order; free
    /// space only with a known window
    pub parts: Vec<(ContextPart, u64)>,
    /// Where `used` comes from
    pub source: ContextSource,
}

impl ContextUsage {
    /// `hud`'s context against a window of `window` tokens (the model's,
    /// else the last turn's); `None` before anything filled it.
    #[must_use]
    pub fn of(hud: &Hud, window: Option<u64>) -> Option<Self> {
        let view = hud.context.unwrap_or_default();
        let window = window
            .or(view.limit_tokens)
            .or(hud.stats.context_limit)
            .filter(|w| *w > 0);
        let (used, source) = match hud.stats.context_tokens {
            0 => (view.total_tokens, ContextSource::Estimated),
            reported => (reported, ContextSource::Reported),
        };
        if used == 0 {
            return None;
        }
        let mut parts = fixed_parts(&view, used);
        let fixed: u64 = parts.iter().map(|(_, tokens)| tokens).sum();
        parts.push((ContextPart::Messages, used.saturating_sub(fixed)));
        if let Some(window) = window {
            parts.push((ContextPart::Free, window.saturating_sub(used)));
        }
        Some(Self {
            used,
            window,
            parts,
            source,
        })
    }

    /// How full the window is, in percent, when it is known.
    #[must_use]
    pub fn percent(&self) -> Option<u64> {
        self.window
            .map(|window| (self.used * 100 / window).min(100))
    }

    /// `tokens` as a share of the window, in tenths of a percent, or of
    /// what is used when the window isn't known.
    #[must_use]
    pub fn share_permille(&self, tokens: u64) -> u64 {
        let of = self.window.unwrap_or(self.used).max(1);
        tokens.saturating_mul(1000) / of
    }
}

/// The parts measured from their text, scaled down together when they
/// come to more than `used` (the estimate runs ahead of a cached call).
fn fixed_parts(view: &ContextView, used: u64) -> Vec<(ContextPart, u64)> {
    let measured = [
        (ContextPart::System, view.system_tokens),
        (ContextPart::Tools, view.tools_tokens),
        (ContextPart::Mcp, view.mcp_tokens),
        (ContextPart::Subagents, view.subagents_tokens),
        (ContextPart::Skills, view.skills_tokens),
    ];
    let total: u64 = measured.iter().map(|(_, tokens)| tokens).sum();
    measured
        .into_iter()
        .map(|(part, tokens)| {
            let tokens = if total > used {
                // u128: tokens * used can pass u64 for huge windows.
                u64::try_from(u128::from(tokens) * u128::from(used) / u128::from(total))
                    .unwrap_or(tokens)
            } else {
                tokens
            };
            (part, tokens)
        })
        .collect()
}

/// The ring for a window `percent` full: an empty circle, a quarter,
/// half, three quarters, or a full one.
#[must_use]
pub const fn ring(percent: u64) -> &'static str {
    match percent {
        0..=12 => "○",
        13..=37 => "◔",
        38..=62 => "◑",
        63..=87 => "◕",
        _ => "●",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hud(reported: u64, view: ContextView) -> Hud {
        let mut hud = Hud::default();
        hud.stats.context_tokens = reported;
        hud.context = Some(view);
        hud
    }

    fn view() -> ContextView {
        ContextView {
            system_tokens: 3_000,
            tools_tokens: 8_000,
            mcp_tokens: 1_000,
            subagents_tokens: 500,
            skills_tokens: 500,
            conversation_tokens: 20_000,
            total_tokens: 33_000,
            limit_tokens: Some(200_000),
        }
    }

    fn tokens(usage: &ContextUsage, part: ContextPart) -> u64 {
        usage
            .parts
            .iter()
            .find(|(p, _)| *p == part)
            .map_or(0, |(_, t)| *t)
    }

    #[test]
    fn the_reported_total_is_split_into_its_parts_and_messages_are_the_rest() {
        let usage = ContextUsage::of(&hud(50_000, view()), None).unwrap();
        assert_eq!(usage.source, ContextSource::Reported);
        assert_eq!((usage.used, usage.window), (50_000, Some(200_000)));
        assert_eq!(tokens(&usage, ContextPart::Tools), 8_000);
        assert_eq!(tokens(&usage, ContextPart::Messages), 37_000);
        assert_eq!(tokens(&usage, ContextPart::Free), 150_000);
        let parts: u64 = usage.parts.iter().map(|(_, t)| t).sum();
        assert_eq!(parts, 200_000, "the parts fill the window exactly");
        assert_eq!(usage.percent(), Some(25));
    }

    #[test]
    fn before_a_call_is_reported_the_loops_estimate_stands_in() {
        let usage = ContextUsage::of(&hud(0, view()), None).unwrap();
        assert_eq!(usage.source, ContextSource::Estimated);
        assert_eq!(usage.used, 33_000);
        assert_eq!(tokens(&usage, ContextPart::Messages), 20_000);
    }

    #[test]
    fn parts_measured_past_the_total_are_scaled_to_fit() {
        let usage = ContextUsage::of(&hud(6_500, view()), Some(10_000)).unwrap();
        let fixed: u64 = usage.parts[..5].iter().map(|(_, t)| t).sum();
        assert!(fixed <= 6_500, "{usage:?}");
        assert_eq!(tokens(&usage, ContextPart::Tools), 4_000);
        assert_eq!(tokens(&usage, ContextPart::Free), 3_500);
    }

    #[test]
    fn nothing_used_is_no_usage_and_an_unknown_window_has_no_free_space() {
        assert_eq!(ContextUsage::of(&Hud::default(), None), None);
        let mut no_window = view();
        no_window.limit_tokens = None;
        let usage = ContextUsage::of(&hud(40_000, no_window), None).unwrap();
        assert_eq!(usage.percent(), None);
        assert!(usage.parts.iter().all(|(p, _)| *p != ContextPart::Free));
        assert_eq!(usage.share_permille(20_000), 500);
    }

    #[test]
    fn parts_come_in_the_panels_order() {
        let usage = ContextUsage::of(&hud(50_000, view()), None).unwrap();
        let order: Vec<ContextPart> = usage.parts.iter().map(|(p, _)| *p).collect();
        assert_eq!(order, ContextPart::ALL);
    }

    #[test]
    fn the_ring_fills_by_quarters() {
        let rings: Vec<&str> = [0, 12, 13, 37, 50, 63, 87, 88, 100]
            .into_iter()
            .map(ring)
            .collect();
        assert_eq!(rings, ["○", "○", "◔", "◔", "◑", "◕", "◕", "●", "●"]);
    }
}
