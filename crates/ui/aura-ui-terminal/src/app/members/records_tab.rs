//! The tabs of the right-hand panel above the side panel: the Kernel
//! panel's, or, while a channel is open, the channel's TASKS.

use super::super::tab_bar::TabLayout;

/// Views of the right-hand panel above the side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordsTab {
    /// The kernel's records of the agent on screen.
    #[default]
    Kernel,
    /// The agents the run of the agent on screen brought in.
    Members,
    /// The policy the kernel of the agent on screen enforces.
    Policies,
    /// The messages the agents of that run sent each other, by thread.
    Inbox,
    /// Every plan and task in the open channel's project, by every agent
    /// working there, as the side panel's TASKS tab lists them. Listed
    /// only while a channel is open.
    ChannelTasks,
}

impl RecordsTab {
    /// The Kernel panel's tabs, listed while no channel is open.
    pub const ALL: [Self; 4] = [Self::Kernel, Self::Members, Self::Policies, Self::Inbox];

    /// The tabs listed while a channel is open: the channel's own, and
    /// none of the Kernel panel's.
    const IN_CHANNEL: [Self; 1] = [Self::ChannelTasks];

    /// The tabs the header bar lists, in order, with a channel open or
    /// not.
    #[must_use]
    pub const fn tabs(channel_open: bool) -> &'static [Self] {
        if channel_open {
            &Self::IN_CHANNEL
        } else {
            &Self::ALL
        }
    }

    /// Whether this is one of a channel's own tabs, listed only while a
    /// channel is open.
    #[must_use]
    pub const fn is_channel_tab(self) -> bool {
        matches!(self, Self::ChannelTasks)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kernel => "KERNEL",
            Self::Members => "MEMBERS",
            Self::Policies => "POLICIES",
            Self::Inbox => "INBOX",
            Self::ChannelTasks => "TASKS",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Kernel => "KRN",
            Self::Members => "MEM",
            Self::Policies => "POL",
            Self::Inbox => "INB",
            Self::ChannelTasks => "TSK",
        }
    }

    /// The tab after this one in [`Self::tabs`], wrapping around, as
    /// Shift+Right goes. A tab that is not listed goes to the first.
    pub(in crate::app) fn next(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let next = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(0, |at| (at + 1) % tabs.len());
        tabs.get(next).copied().unwrap_or_default()
    }

    /// The tab before this one in [`Self::tabs`], wrapping around, as
    /// Shift+Left goes. A tab that is not listed goes to the last.
    pub(in crate::app) fn prev(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let prev = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(tabs.len() - 1, |at| (at + tabs.len() - 1) % tabs.len());
        tabs.get(prev).copied().unwrap_or_default()
    }

    /// The tabs' cells in a header bar `width` columns wide, with the open
    /// tab's count after them, with a channel open or not. The renderer
    /// draws the bar with this layout and clicks are mapped back through
    /// it.
    #[must_use]
    pub fn layout(width: u16, channel_open: bool) -> TabLayout<Self> {
        if channel_open {
            TabLayout::with_info(Self::IN_CHANNEL, Self::label, Self::short_label, width)
        } else {
            TabLayout::with_info(Self::ALL, Self::label, Self::short_label, width)
        }
    }
}
