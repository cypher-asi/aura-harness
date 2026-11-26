//! The tabs of the right-hand panel below the Record panel.

use super::super::tab_bar::TabLayout;

/// Views of the right-hand panel below the Record panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidePanelTab {
    /// The open channel's members: its coordinator and the agents it
    /// spawned. Listed only while a channel is open, first.
    ChannelMembers,
    /// The open channel's description, coordinator and project.
    /// Listed only while a channel is open.
    ChannelInfo,
    /// File tree of the active agent's workspace.
    #[default]
    Sandbox,
    /// Live agent turns, spawned agents and background commands.
    Processes,
    /// The host the agent on screen runs on, this machine or its cloud
    /// machine: resources, addresses, status and cost.
    Host,
    /// Live feed of recorded steps and harness decisions.
    Trace,
    /// Work agents did or are doing, grouped under the plans they work
    /// on, each plan with its todos and each task with what is done and
    /// left.
    Tasks,
}

impl SidePanelTab {
    /// The tabs listed while no channel is open: the agent app's.
    const OUTSIDE_CHANNEL: [Self; 5] = [
        Self::Sandbox,
        Self::Processes,
        Self::Host,
        Self::Trace,
        Self::Tasks,
    ];

    /// The tabs listed while a channel is open: the channel's own, and
    /// none of the agent app's.
    const IN_CHANNEL: [Self; 2] = [Self::ChannelMembers, Self::ChannelInfo];

    /// The tabs the header bar lists, in order, with a channel open or
    /// not.
    #[must_use]
    pub const fn tabs(channel_open: bool) -> &'static [Self] {
        if channel_open {
            &Self::IN_CHANNEL
        } else {
            &Self::OUTSIDE_CHANNEL
        }
    }

    /// Whether this is one of a channel's own tabs, listed only while a
    /// channel is open.
    #[must_use]
    pub const fn is_channel_tab(self) -> bool {
        matches!(self, Self::ChannelMembers | Self::ChannelInfo)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MEMBERS",
            Self::ChannelInfo => "INFO",
            Self::Sandbox => "SANDBOX",
            Self::Processes => "PROCESSES",
            Self::Host => "HOST",
            Self::Trace => "TRACE",
            Self::Tasks => "TASKS",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MBR",
            Self::ChannelInfo => "INF",
            Self::Sandbox => "SBX",
            Self::Processes => "PRC",
            Self::Host => "HST",
            Self::Trace => "TRC",
            Self::Tasks => "TSK",
        }
    }

    /// The tabs' cells in a header bar `width` columns wide, as drawn and
    /// as clicks are mapped back, with a channel open or not.
    #[must_use]
    pub fn layout(width: u16, channel_open: bool) -> TabLayout<Self> {
        if channel_open {
            TabLayout::new(Self::IN_CHANNEL, Self::label, Self::short_label, width)
        } else {
            TabLayout::new(Self::OUTSIDE_CHANNEL, Self::label, Self::short_label, width)
        }
    }

    /// The tab after this one in [`Self::tabs`], wrapping around. A tab
    /// that is not listed goes to the first.
    pub(in crate::app) fn next(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let next = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(0, |at| (at + 1) % tabs.len());
        tabs.get(next).copied().unwrap_or_default()
    }

    /// The tab before this one in [`Self::tabs`], wrapping around. A tab
    /// that is not listed goes to the last.
    pub(in crate::app) fn prev(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let prev = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(tabs.len() - 1, |at| (at + tabs.len() - 1) % tabs.len());
        tabs.get(prev).copied().unwrap_or_default()
    }
}
