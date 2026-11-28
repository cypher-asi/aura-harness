//! What an empty channel's chat shows: its `#slug` as a banner over a
//! welcome from its coordinator.
//!
//! The welcome is drawn, not posted: it lives only on screen while the
//! channel has nothing in it, so it never reaches the coordinator, costs
//! no model call and is not stored. The first message takes its place.

use crate::app::App;
use crate::events::ChannelSummary;

/// The welcome an empty channel's chat shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelWelcome {
    /// The slug the banner spells
    pub(crate) slug: String,
    /// What the coordinator says, a paragraph each
    pub(crate) paragraphs: Vec<String>,
    /// A dimmer line under it, on how to reach a member
    pub(crate) hint: String,
}

/// How a member is reached directly from the channel's chat.
const MEMBER_HINT: &str = "Mention a member with @CODE to talk to it directly.";

impl ChannelWelcome {
    /// The welcome of `channel`: what it is for, from its description or
    /// else its title, and how to get started, signed by its coordinator.
    pub(crate) fn of(channel: &ChannelSummary) -> Self {
        let slug = channel.slug.clone();
        let about = [&channel.description, &channel.title]
            .into_iter()
            .map(|text| text.trim())
            .find(|text| !text.is_empty() && *text != slug);
        let greeting = match about {
            Some(about) => format!("Welcome to #{slug}: {}", sentence(about)),
            None => format!("Welcome to #{slug}."),
        };
        let start = match coordinator_name(channel) {
            Some(name) => format!(
                "I'm {name}, the coordinator. Tell me what you need and I'll hand it to an \
                 agent, then report back here."
            ),
            None => "I'm the coordinator. Tell me what you need and I'll hand it to an agent, \
                     then report back here."
                .to_string(),
        };
        Self {
            slug,
            paragraphs: vec![greeting, start],
            hint: MEMBER_HINT.to_string(),
        }
    }
}

/// `text` ending with a full stop unless it already ends a sentence.
fn sentence(text: &str) -> String {
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// What `channel`'s coordinator is called in its chat: its code, else
/// its name.
fn coordinator_name(channel: &ChannelSummary) -> Option<String> {
    let coordinator = channel
        .members
        .iter()
        .find(|member| member.id == channel.coordinator_id)?;
    coordinator
        .code
        .clone()
        .filter(|code| !code.trim().is_empty())
        .or_else(|| Some(coordinator.name.clone()).filter(|name| !name.trim().is_empty()))
}

impl App {
    /// The welcome the chat shows while the open channel's own chat has
    /// nothing in it: no post and no message. `None` elsewhere, over a
    /// member's chat, and in the terminal.
    #[must_use]
    pub(crate) fn channel_welcome(&self) -> Option<ChannelWelcome> {
        if self.terminal_mode || !self.channel_app() || !self.pane_is_empty() {
            return None;
        }
        self.coordinated_channel().map(ChannelWelcome::of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AgentSummary;

    fn channel(description: &str, title: &str, code: Option<&str>) -> ChannelSummary {
        ChannelSummary {
            id: "c1".to_string(),
            slug: "aura".to_string(),
            title: title.to_string(),
            description: description.to_string(),
            coordinator_id: "co".to_string(),
            members: vec![AgentSummary {
                id: "co".to_string(),
                name: "Coordinator".to_string(),
                code: code.map(str::to_string),
                ..AgentSummary::default()
            }],
            ..ChannelSummary::default()
        }
    }

    #[test]
    fn the_welcome_says_what_the_channel_is_for_and_how_to_start() {
        let welcome = ChannelWelcome::of(&channel(
            "agentic harness for swarm super intelligence",
            "Aura",
            Some("COO2"),
        ));
        assert_eq!(welcome.slug, "aura");
        assert_eq!(
            welcome.paragraphs,
            [
                "Welcome to #aura: agentic harness for swarm super intelligence.",
                "I'm COO2, the coordinator. Tell me what you need and I'll hand it to an \
                 agent, then report back here.",
            ]
        );
        assert_eq!(welcome.hint, MEMBER_HINT);
    }

    #[test]
    fn without_a_description_the_title_says_what_it_is_for() {
        let welcome = ChannelWelcome::of(&channel("  ", "Website relaunch", None));
        assert_eq!(welcome.paragraphs[0], "Welcome to #aura: Website relaunch.");
        assert!(
            welcome.paragraphs[1].starts_with("I'm Coordinator, the coordinator."),
            "{}",
            welcome.paragraphs[1]
        );
        let bare = ChannelWelcome::of(&channel("", "aura", None));
        assert_eq!(bare.paragraphs[0], "Welcome to #aura.");
        let finished = ChannelWelcome::of(&channel("Ship it!", "", None));
        assert_eq!(finished.paragraphs[0], "Welcome to #aura: Ship it!");
    }

    #[test]
    fn a_coordinator_not_among_the_members_is_just_the_coordinator() {
        let mut nobody = channel("", "", None);
        nobody.members.clear();
        let welcome = ChannelWelcome::of(&nobody);
        assert!(
            welcome.paragraphs[1].starts_with("I'm the coordinator. Tell me"),
            "{}",
            welcome.paragraphs[1]
        );
    }
}
