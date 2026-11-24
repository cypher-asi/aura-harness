//! What the cloud fleet records about the machines agents run on, and
//! where an agent runs.
//!
//! - [`VmPayload`] is the payload of a [`SystemKind::Vm`] system
//!   transaction. The fleet writes one per machine state change, and a
//!   `usage` row with boxd's meter reading every 15 minutes, on a ledger
//!   agent of its own per cloud agent (`0://cloud/vm/<agent zns>`). The
//!   latest rows are the machine as it stands; the `usage` rows are what
//!   it cost.
//! - [`PlacementPayload`] is the payload of a [`SystemKind::Placement`]
//!   transaction on the agent's own record, written at the moment its
//!   single writer moves between this machine and a cloud machine.
//!
//! Tokens and keys are never part of either payload.

use serde::{Deserialize, Serialize};

use super::transaction::SystemKind;

/// Where an agent's kernel runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// On this machine, in the local app.
    #[default]
    Local,
    /// On a boxd machine of its own.
    Cloud,
}

impl Placement {
    /// `local` or `cloud`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud => "cloud",
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

    /// Parse `local` / `cloud`, case-insensitively.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "cloud" => Some(Self::Cloud),
            _ => None,
        }
    }
}

impl std::fmt::Display for Placement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Payload of a [`SystemKind::Placement`] transaction: the agent's
/// writer moved from `from` to `to` at this entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementPayload {
    /// Always [`SystemKind::Placement`].
    pub system_kind: SystemKind,
    /// Where the agent ran before this entry.
    pub from: Placement,
    /// Where it runs from this entry on.
    pub to: Placement,
    /// The machine it runs on from now (cloud) or ran on until now
    /// (local), by boxd name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl PlacementPayload {
    /// A move from `from` to `to`.
    #[must_use]
    pub const fn new(from: Placement, to: Placement, machine: Option<String>) -> Self {
        Self {
            system_kind: SystemKind::Placement,
            from,
            to,
            machine,
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        // Enums and strings always serialize.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Read a stored payload; `None` for any other system payload.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|payload| payload.system_kind == SystemKind::Placement)
    }
}

/// What happened to a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmEvent {
    /// Created from a snapshot or an image.
    Created,
    /// Created as a fork of another machine.
    Forked,
    /// Its node took this hub's token and node id.
    Claimed,
    /// Went to standby (memory kept in RAM).
    Standby,
    /// Hibernated (memory written to disk; disk-only billing).
    Hibernated,
    /// Came back to running from standby or hibernation.
    Woke,
    /// Changed size.
    Resized,
    /// A snapshot was saved from it.
    SnapshotSaved,
    /// boxd's meter reading for the machine.
    Usage,
    /// Destroyed; carries the final meter reading once boxd has it.
    Destroyed,
    /// An operation on it failed.
    Error,
}

impl VmEvent {
    /// The event's name, as stored.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Forked => "forked",
            Self::Claimed => "claimed",
            Self::Standby => "standby",
            Self::Hibernated => "hibernated",
            Self::Woke => "woke",
            Self::Resized => "resized",
            Self::SnapshotSaved => "snapshot_saved",
            Self::Usage => "usage",
            Self::Destroyed => "destroyed",
            Self::Error => "error",
        }
    }
}

/// What a machine was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VmSource {
    /// Restored from a snapshot.
    Snapshot {
        /// Snapshot name.
        name: String,
        /// Its version at the time, when boxd reported one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
    },
    /// Forked from another machine.
    Fork {
        /// boxd name of the parent machine.
        parent: String,
    },
    /// Booted fresh from an image (snapshot builders).
    Image {
        /// Image reference; empty for boxd's default image.
        image: String,
    },
}

/// The resources a machine was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmResources {
    /// vCPUs.
    pub vcpu: u32,
    /// Memory, in bytes.
    pub memory_bytes: u64,
    /// Disk, in bytes (boxd's fixed copy-on-write disk).
    pub disk_bytes: u64,
}

/// One meter reading from boxd for a machine, over its life so far.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct VmUsage {
    /// Active vCPU-hours.
    pub vcpu_hours: f64,
    /// GiB-hours of memory in use.
    pub ram_gib_hours: f64,
    /// GiB-hours of disk written.
    pub disk_gib_hours: f64,
    /// What it cost, in micro-units (1e-6) of `currency`.
    pub cost_micro: i64,
    /// Whether every second up to `until_ms` is metered, so the figure
    /// will not grow. A machine that still exists is never complete.
    pub complete: bool,
    /// End of the metered window, Unix milliseconds.
    pub until_ms: u64,
}

impl VmUsage {
    /// The cost in whole units of the currency.
    #[must_use]
    pub fn cost(&self) -> f64 {
        // Costs fit well within f64's exact integer range.
        #[allow(clippy::cast_precision_loss)]
        let micro = self.cost_micro as f64;
        micro / 1_000_000.0
    }
}

/// Payload of a [`SystemKind::Vm`] transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VmPayload {
    /// Always [`SystemKind::Vm`].
    pub system_kind: SystemKind,
    /// What happened.
    pub event: VmEvent,
    /// boxd machine id.
    pub machine_id: String,
    /// boxd machine name (its domain is `<name>.boxd.sh`).
    pub machine_name: String,
    /// Region the machine lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Its resources, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<VmResources>,
    /// Whether it was created isolated.
    #[serde(default)]
    pub isolated: bool,
    /// What it was made from (on `created` / `forked`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<VmSource>,
    /// boxd status after the event (`running`, `suspended`,
    /// `hibernated`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// How long it took to come up, for `created` / `forked` / `woke`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_ms: Option<u64>,
    /// Why it happened (`idle`, `archived`, an error message, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The meter reading, on `usage` and `destroyed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<VmUsage>,
    /// Currency of `usage` (`eur` or `usd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Snapshot name, on `snapshot_saved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

impl VmPayload {
    /// A bare `event` row for a machine; fill the rest with struct
    /// update syntax.
    #[must_use]
    pub fn new(event: VmEvent, machine_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            system_kind: SystemKind::Vm,
            event,
            machine_id: machine_id.into(),
            machine_name: name.into(),
            region: None,
            resources: None,
            isolated: false,
            source: None,
            status: None,
            boot_ms: None,
            reason: None,
            usage: None,
            currency: None,
            snapshot: None,
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        // Strings, numbers and enums always serialize; a non-finite
        // float would only come from a meter bug and is stored as null.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Read a stored payload; `None` for any other system payload.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|payload| payload.system_kind == SystemKind::Vm)
    }
}

/// Rates the live cost estimate prices resources at, per hour, in
/// `currency`. Defaults are boxd's list prices (euro).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceRates {
    /// Per active vCPU-hour (idle reserved vCPUs are free).
    pub vcpu_hour: f64,
    /// Per GiB-hour of memory in use.
    pub mem_gib_hour: f64,
    /// Per GiB-hour of disk written.
    pub disk_gib_hour: f64,
    /// `eur` or `usd`.
    pub currency: String,
}

impl PriceRates {
    /// boxd's list price per active vCPU-hour, in euro.
    pub const BOXD_VCPU_HOUR: f64 = 0.039;
    /// boxd's list price per GiB-hour of memory in use, in euro.
    pub const BOXD_MEM_GIB_HOUR: f64 = 0.012;
    /// boxd's list price per GiB-hour of disk written, in euro.
    pub const BOXD_DISK_GIB_HOUR: f64 = 0.000_08;

    /// boxd's list prices.
    #[must_use]
    pub fn boxd_list() -> Self {
        Self {
            vcpu_hour: Self::BOXD_VCPU_HOUR,
            mem_gib_hour: Self::BOXD_MEM_GIB_HOUR,
            disk_gib_hour: Self::BOXD_DISK_GIB_HOUR,
            currency: "eur".to_string(),
        }
    }

    /// The cost of `usage`-shaped amounts at these rates.
    #[must_use]
    pub fn price(&self, vcpu_hours: f64, ram_gib_hours: f64, disk_gib_hours: f64) -> f64 {
        vcpu_hours * self.vcpu_hour
            + ram_gib_hours * self.mem_gib_hour
            + disk_gib_hours * self.disk_gib_hour
    }
}

impl Default for PriceRates {
    fn default() -> Self {
        Self::boxd_list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_payload_round_trips() {
        let payload = VmPayload {
            region: Some("nyc".into()),
            resources: Some(VmResources {
                vcpu: 4,
                memory_bytes: 16 << 30,
                disk_bytes: 100 << 30,
            }),
            isolated: true,
            source: Some(VmSource::Snapshot {
                name: "aura-proj-x".into(),
                version: Some(4),
            }),
            boot_ms: Some(6),
            usage: Some(VmUsage {
                vcpu_hours: 0.5,
                ram_gib_hours: 4.0,
                disk_gib_hours: 30.0,
                cost_micro: 410_000,
                complete: false,
                until_ms: 1_700_000_000_000,
            }),
            currency: Some("eur".into()),
            ..VmPayload::new(VmEvent::Created, "id-1", "aura-re36-3f9c")
        };
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["system_kind"], "vm");
        assert_eq!(json["event"], "created");
        assert_eq!(json["source"]["kind"], "snapshot");
        assert_eq!(VmPayload::from_bytes(&bytes), Some(payload));
    }

    #[test]
    fn other_system_payloads_are_not_vm_rows() {
        let rewind = serde_json::json!({"system_kind": "rewind", "turn": 1});
        let bytes = serde_json::to_vec(&rewind).expect("json");
        assert_eq!(VmPayload::from_bytes(&bytes), None);
        assert_eq!(PlacementPayload::from_bytes(&bytes), None);
    }

    #[test]
    fn placement_payload_round_trips() {
        let payload = PlacementPayload::new(
            Placement::Local,
            Placement::Cloud,
            Some("aura-re36-3f9c".into()),
        );
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["system_kind"], "placement");
        assert_eq!(json["to"], "cloud");
        assert_eq!(PlacementPayload::from_bytes(&bytes), Some(payload));
    }

    #[test]
    fn placement_parses_and_flips() {
        assert_eq!(Placement::parse(" Cloud "), Some(Placement::Cloud));
        assert_eq!(Placement::parse("remote"), None);
        assert_eq!(Placement::Local.flipped(), Placement::Cloud);
        assert_eq!(Placement::default(), Placement::Local);
    }

    #[test]
    fn usage_cost_is_in_whole_units() {
        let usage = VmUsage {
            cost_micro: 2_180_000,
            ..VmUsage::default()
        };
        assert!((usage.cost() - 2.18).abs() < 1e-9);
    }

    #[test]
    fn list_prices_price_an_hour_of_a_busy_default_machine() {
        let rates = PriceRates::boxd_list();
        // 4 vCPU busy, 16 GiB in use, 100 GiB written, for one hour.
        let cost = rates.price(4.0, 16.0, 100.0);
        assert!((cost - (0.156 + 0.192 + 0.008)).abs() < 1e-9, "{cost}");
        assert_eq!(rates.currency, "eur");
    }
}
