//! What the HOST tab and the footer's placement switch show: the host an
//! agent runs on, local or cloud, its resources, addresses, status and,
//! for cloud machines, its cost.
//!
//! Built by the app ([`UiCommand::SetHost`](crate::events::UiCommand))
//! and drawn by any front end; nothing here reads the system itself.

use serde::{Deserialize, Serialize};

/// Where an agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementView {
    /// On this machine.
    #[default]
    Local,
    /// On its own cloud machine.
    Cloud,
}

impl PlacementView {
    /// The footer label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud => "cloud",
        }
    }

    /// The mark before the footer's label and after a cloud agent's name.
    /// The cloud's second column is blank: one column is too narrow to
    /// make a cloud out, so the desktop window draws it across both.
    #[must_use]
    pub const fn mark(self) -> &'static str {
        match self {
            Self::Local => "⌂",
            Self::Cloud => "☁ ",
        }
    }

    /// The other placement.
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Local => Self::Cloud,
            Self::Cloud => Self::Local,
        }
    }
}

/// The footer switch of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlacementState {
    /// Where the agent runs.
    pub placement: PlacementView,
    /// A move under way (`moving to cloud`, `pulling changes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<String>,
    /// The machine's state when in the cloud (`ready`, `asleep`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_state: Option<String>,
    /// Whether cloud mode is set up (a boxd key is loaded).
    pub cloud_ready: bool,
    /// The user who placed the agent, when it is not the one signed in
    /// here: only they can move or manage it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// One labelled fact in the HOST tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFact {
    /// Label (`CPU`, `Memory`, `IP`).
    pub label: String,
    /// Value as shown.
    pub value: String,
    /// A share 0-100 to draw as a bar, when the fact is a use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// Near its limit: the value and its bar are shown in the accent.
    #[serde(default)]
    pub warn: bool,
}

impl HostFact {
    /// A plain fact.
    #[must_use]
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            percent: None,
            warn: false,
        }
    }

    /// A fact with a use bar.
    #[must_use]
    pub fn with_percent(mut self, percent: f64) -> Self {
        let clamped = percent.clamp(0.0, 100.0).round();
        // In 0..=100 after the clamp.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let value = clamped as u8;
        self.percent = Some(value);
        self.warn = value >= 90;
        self
    }
}

/// A titled group of facts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostSection {
    /// Title (`MACHINE`, `RESOURCES`, `NETWORK`, `COST`, `EVENTS`).
    pub title: String,
    /// Its facts.
    pub facts: Vec<HostFact>,
}

/// One host in the project view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRow {
    /// Agent, hex.
    pub agent_id: String,
    /// Agent's display name.
    pub agent: String,
    /// Host name (`this machine`, `aura-re36-3f9c`).
    pub host: String,
    /// State label.
    pub state: String,
    /// Cost per hour as shown, or empty for local.
    pub per_hour: String,
    /// Cost so far as shown, or empty for local.
    pub total: String,
    /// The user whose plan and money the agent spends, when another
    /// user's agents share the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// Buttons the HOST tab offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAction {
    /// Pull the cloud agent's changes into the project.
    Pull,
    /// Put the machine to sleep now.
    Sleep,
    /// Wake the machine.
    Wake,
    /// Move the agent to the other placement.
    Flip,
}

impl HostAction {
    /// Button label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pull => "pull changes",
            Self::Sleep => "sleep",
            Self::Wake => "wake",
            Self::Flip => "move",
        }
    }

    /// Key that works it in the tab.
    #[must_use]
    pub const fn key(self) -> char {
        match self {
            Self::Pull => 'p',
            Self::Sleep => 's',
            Self::Wake => 'w',
            Self::Flip => 'm',
        }
    }
}

/// Everything the HOST tab shows for the agent on screen.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostView {
    /// Agent, hex.
    pub agent_id: String,
    /// Where it runs.
    pub placement: PlacementView,
    /// Headline (`this machine · n3o-desk`, `boxd · aura-re36-3f9c`).
    pub title: String,
    /// Status line (`● ready · up 2h 14m · sleeps in 4m`).
    pub status: String,
    /// Groups of facts.
    pub sections: Vec<HostSection>,
    /// Buttons that apply now.
    pub actions: Vec<HostAction>,
    /// Every host of the project, for the project scope.
    pub project: Vec<HostRow>,
    /// Project totals (`3 cloud machines · €0.42/h est. · €1.80 today`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_total: Option<String>,
    /// When the facts were sampled, `HH:MM:SS`; shown for sleeping
    /// machines, whose facts are the last known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
}

/// Bytes as `12.3 GiB`.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Seconds as `2h 14m`, `4m`, `35s`, `3d 2h`.
#[must_use]
pub fn format_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

/// An amount with its currency: `€0.42`, `$1.20`, `0.42 GBP`.
#[must_use]
pub fn format_money(amount: f64, currency: &str) -> String {
    let digits = if amount.abs() < 0.1 { 4 } else { 2 };
    match currency.to_ascii_lowercase().as_str() {
        "eur" => format!("€{amount:.digits$}"),
        "usd" => format!("${amount:.digits$}"),
        other => format!("{amount:.digits$} {}", other.to_ascii_uppercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(16 * 1024 * 1024 * 1024), "16.0 GiB");
        assert_eq!(format_duration(35), "35s");
        assert_eq!(format_duration(8040), "2h 14m");
        assert_eq!(format_duration(3 * 86_400 + 7200), "3d 2h");
        assert_eq!(format_money(0.42, "eur"), "€0.42");
        assert_eq!(format_money(0.0016, "eur"), "€0.0016");
        assert_eq!(format_money(1.2, "usd"), "$1.20");
    }

    #[test]
    fn percent_facts_warn_when_nearly_full() {
        let fact = HostFact::new("Disk", "95 GiB").with_percent(95.4);
        assert_eq!(fact.percent, Some(95));
        assert!(fact.warn);
        assert!(!HostFact::new("CPU", "4").with_percent(20.0).warn);
    }

    #[test]
    fn placements_flip_and_label() {
        assert_eq!(PlacementView::Local.flipped(), PlacementView::Cloud);
        assert_eq!(PlacementView::Cloud.label(), "cloud");
        assert_eq!(PlacementView::default(), PlacementView::Local);
    }
}
