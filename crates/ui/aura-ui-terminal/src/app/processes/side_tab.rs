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
    /// The merges of the open channel's merge queue, newest first.
    /// Listed only while a channel is open, after INFO.
    ChannelQueue,
    /// File tree of the active agent's workspace.
    #[default]
    Sandbox,
    /// The git repo the active agent works in: its uncommitted changes, a
    /// timeline of its commits, pushes, pulls and PRs, and its branches,
    /// tags and worktrees.
    Repo,
    /// Live agent turns, spawned agents and background commands.
    Processes,
    /// The host the agent on screen runs on, this machine or its cloud
    /// machine: resources, addresses, status and cost. Its tab is an
    /// icon at the bar's end.
    Host,
    /// Live feed of recorded steps and harness decisions.
    Trace,
    /// Work agents did or are doing, grouped under the plans they work
    /// on, each plan with its todos and each task with what is done and
    /// left. Listed in a channel too, last, where it lists the whole
    /// project's work.
    Tasks,
    /// Every message the user sent in the open channel, newest first.
    /// Listed only while a channel is open, last, after TASKS.
    ChannelHistory,
}

impl SidePanelTab {
    /// The tabs listed while no channel is open: the agent app's, HOST's
    /// icon last.
    const OUTSIDE_CHANNEL: [Self; 6] = [
        Self::Sandbox,
        Self::Repo,
        Self::Processes,
        Self::Trace,
        Self::Tasks,
        Self::Host,
    ];

    /// The tabs of [`Self::OUTSIDE_CHANNEL`] the bar labels; HOST is an
    /// icon after them.
    const LABELLED: [Self; 5] = [
        Self::Sandbox,
        Self::Repo,
        Self::Processes,
        Self::Trace,
        Self::Tasks,
    ];

    /// The tabs listed while a channel is open: the channel's own, then
    /// TASKS, then HISTORY, and none of the agent app's others.
    const IN_CHANNEL: [Self; 5] = [
        Self::ChannelMembers,
        Self::ChannelInfo,
        Self::ChannelQueue,
        Self::Tasks,
        Self::ChannelHistory,
    ];

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
        matches!(
            self,
            Self::ChannelMembers | Self::ChannelInfo | Self::ChannelQueue | Self::ChannelHistory
        )
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MEMBERS",
            Self::ChannelInfo => "INFO",
            Self::ChannelQueue => "QUEUE",
            Self::Sandbox => "SANDBOX",
            Self::Repo => "REPO",
            Self::Processes => "PROCESSES",
            Self::Host => "HOST",
            Self::Trace => "TRACE",
            Self::Tasks => "TASKS",
            Self::ChannelHistory => "HISTORY",
        }
    }

    /// What a tab drawn as an icon shows instead of its label: HOST's
    /// computer. Its second column is blank: one column is too narrow to
    /// make a computer out, so the desktop window draws it across both.
    #[must_use]
    pub const fn icon(self) -> Option<&'static str> {
        match self {
            Self::Host => Some("🖥 "),
            _ => None,
        }
    }

    /// The tab the bar draws as an icon at its end, with a channel open
    /// or not.
    #[must_use]
    pub const fn icon_tab(channel_open: bool) -> Option<Self> {
        if channel_open {
            None
        } else {
            Some(Self::Host)
        }
    }

    /// The name shown beside a tab drawn as an icon while the pointer is
    /// over it, which its icon alone does not spell out.
    #[must_use]
    pub const fn tooltip(self) -> Option<&'static str> {
        match self {
            Self::Host => Some("Host"),
            _ => None,
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MBR",
            Self::ChannelInfo => "INF",
            Self::ChannelQueue => "QUE",
            Self::Sandbox => "SBX",
            Self::Repo => "RPO",
            Self::Processes => "PRC",
            Self::Host => "HST",
            Self::Trace => "TRC",
            Self::Tasks => "TSK",
            Self::ChannelHistory => "HIS",
        }
    }

    /// The labelled tabs' cells in a header bar `width` columns wide, as
    /// drawn and as clicks are mapped back, with a channel open or not.
    #[must_use]
    pub fn layout(width: u16, channel_open: bool) -> TabLayout<Self> {
        if channel_open {
            TabLayout::new(Self::IN_CHANNEL, Self::label, Self::short_label, width)
        } else {
            TabLayout::new(Self::LABELLED, Self::label, Self::short_label, width)
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
