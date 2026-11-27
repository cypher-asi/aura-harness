//! The agents panel's tabs: which app is in front, how its header bar
//! lays them out, and the order keys move through them.

use super::super::tab_bar::TabLayout;

/// Views of the agents panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsTab {
    /// Channels: shared conversations with a coordinator and its agents.
    Channels,
    /// Live agents: the ones the user talks to and the instances they spawned.
    #[default]
    Projects,
    /// The roster of permanent agents that can be spawned by name.
    Roster,
    /// Benchmarks: the same work on two arms (Gate or a model), their
    /// cost and how well they did. Its tab is an icon at the bar's end.
    Bench,
    /// Agents moved out of the projects list, until restored.
    Archive,
}

impl AgentsTab {
    /// Every tab, in the order Alt+digit numbers them and Left / Right
    /// move through them. BENCH keeps its place here though its tab is
    /// drawn as an icon after the others, so its keys stay as they were.
    pub const ALL: [Self; 5] = [
        Self::Channels,
        Self::Projects,
        Self::Roster,
        Self::Bench,
        Self::Archive,
    ];

    /// The tabs the header bar labels; BENCH is an icon after them.
    pub const LABELLED: [Self; 4] = [Self::Channels, Self::Projects, Self::Roster, Self::Archive];

    /// The tab the header bar draws as an icon at its end.
    pub const ICON_TAB: Self = Self::Bench;

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Channels => "CHANNELS",
            Self::Projects => "PROJECTS",
            Self::Roster => "AGENTS",
            Self::Bench => "BENCH",
            Self::Archive => "ARCHIVE",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Channels => "CHN",
            Self::Projects => "PRJ",
            Self::Roster => "AGT",
            Self::Bench => "BEN",
            Self::Archive => "ARC",
        }
    }

    /// What a tab drawn as an icon shows instead of its label: BENCH's
    /// wrench, two columns wide. The desktop window draws its own.
    #[must_use]
    pub const fn icon(self) -> Option<&'static str> {
        match self {
            Self::Bench => Some("🔧"),
            _ => None,
        }
    }

    /// The name shown beside a tab drawn as an icon while the pointer is
    /// over it, which its icon alone does not spell out.
    #[must_use]
    pub const fn tooltip(self) -> Option<&'static str> {
        match self {
            Self::Bench => Some("Bench"),
            _ => None,
        }
    }

    /// The next tab (`forward`) or the one before, wrapping, for Left / Right.
    pub(in crate::app) const fn shifted(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::Channels, true) | (Self::Roster, false) => Self::Projects,
            (Self::Projects, true) | (Self::Bench, false) => Self::Roster,
            (Self::Roster, true) | (Self::Archive, false) => Self::Bench,
            (Self::Bench, true) | (Self::Channels, false) => Self::Archive,
            (Self::Archive, true) | (Self::Projects, false) => Self::Channels,
        }
    }

    /// The tab Alt+`digit` brings to the front: 1 to 5, in [`Self::ALL`]'s
    /// order.
    #[must_use]
    pub fn numbered(digit: char) -> Option<Self> {
        let index = digit.to_digit(10)?.checked_sub(1)?;
        Self::ALL.get(usize::try_from(index).ok()?).copied()
    }

    /// The labelled tabs' cells in a header bar `width` columns wide: the
    /// width the bar leaves them beside BENCH's icon. The renderer draws
    /// the bar with this layout and clicks are mapped back through it.
    #[must_use]
    pub fn layout(width: u16) -> TabLayout<Self> {
        TabLayout::new(Self::LABELLED, Self::label, Self::short_label, width)
    }
}
