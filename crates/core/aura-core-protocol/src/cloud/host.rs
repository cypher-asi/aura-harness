//! What a host reports about itself: the machine's name, OS, addresses,
//! CPU, memory and disk. A cloud node serves it at `GET /v1/host` and
//! sends it as [`super::MeshFrame::HostStats`]; the hub fills the same
//! shape for this machine.

use serde::{Deserialize, Serialize};

/// One network address of the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAddress {
    /// Interface name (`eth0`, `en0`, ...).
    pub interface: String,
    /// The address, IPv4 or IPv6, without a prefix length.
    pub ip: String,
    /// Whether it is a private, loopback or link-local address.
    pub private: bool,
}

/// The snapshot a cloud machine was restored from, as written into the
/// machine (`/etc/aura/snapshot.json`) before the snapshot was saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotOrigin {
    /// Snapshot name.
    pub name: String,
    /// Fingerprint of what the snapshot was built from.
    pub fingerprint: String,
    /// Project commit the snapshot holds, when it holds a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// When it was built, Unix milliseconds.
    pub built_ms: u64,
}

/// One sample of a host's vital facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HostStats {
    /// Host name.
    pub hostname: String,
    /// OS name and version (`Ubuntu 24.04`).
    pub os: String,
    /// Kernel version.
    pub kernel: String,
    /// CPU architecture (`x86_64`).
    pub arch: String,
    /// Seconds since the host booted.
    pub uptime_secs: u64,
    /// Logical CPUs.
    pub cpu_count: u32,
    /// CPU busy share over the last sample interval, 0-100 across all
    /// CPUs.
    pub cpu_percent: f32,
    /// Load averages (1, 5, 15 min); zero where the OS has none.
    pub load: [f64; 3],
    /// CPU-seconds the host has spent busy since boot, summed over CPUs:
    /// what boxd bills as active vCPU time.
    pub cpu_busy_secs: f64,
    /// Memory in use, bytes.
    pub mem_used_bytes: u64,
    /// Memory in total, bytes.
    pub mem_total_bytes: u64,
    /// Disk used on the filesystem holding `disk_path`, bytes.
    pub disk_used_bytes: u64,
    /// Size of that filesystem, bytes.
    pub disk_total_bytes: u64,
    /// The path the disk figures are for (the workspace).
    pub disk_path: String,
    /// Network addresses.
    pub addresses: Vec<HostAddress>,
    /// When the sample was taken, Unix milliseconds.
    pub sampled_ms: u64,
    /// The snapshot a cloud machine came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotOrigin>,
}

impl HostStats {
    /// Memory in use as a share of the total, 0-1.
    #[must_use]
    pub fn mem_share(&self) -> f64 {
        share(self.mem_used_bytes, self.mem_total_bytes)
    }

    /// Disk in use as a share of the total, 0-1.
    #[must_use]
    pub fn disk_share(&self) -> f64 {
        share(self.disk_used_bytes, self.disk_total_bytes)
    }

    /// The first private address that is not loopback, if any.
    #[must_use]
    pub fn private_ip(&self) -> Option<&str> {
        self.addresses
            .iter()
            .filter(|a| a.private)
            .map(|a| a.ip.as_str())
            .find(|ip| !ip.starts_with("127.") && *ip != "::1" && !ip.starts_with("fe80"))
    }
}

#[allow(clippy::cast_precision_loss)]
fn share(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_handle_zero_totals() {
        let stats = HostStats {
            mem_used_bytes: 4,
            mem_total_bytes: 16,
            ..HostStats::default()
        };
        assert!((stats.mem_share() - 0.25).abs() < 1e-9);
        assert!(stats.disk_share().abs() < f64::EPSILON);
    }

    #[test]
    fn private_ip_skips_loopback_and_link_local() {
        let addr = |ip: &str, private| HostAddress {
            interface: "eth0".into(),
            ip: ip.into(),
            private,
        };
        let stats = HostStats {
            addresses: vec![
                addr("127.0.0.1", true),
                addr("fe80::1", true),
                addr("10.42.0.17", true),
                addr("34.1.2.3", false),
            ],
            ..HostStats::default()
        };
        assert_eq!(stats.private_ip(), Some("10.42.0.17"));
    }

    #[test]
    fn host_stats_round_trip() {
        let stats = HostStats {
            hostname: "aura-re36".into(),
            cpu_count: 4,
            load: [1.0, 2.0, 3.0],
            snapshot: Some(SnapshotOrigin {
                name: "aura-proj-x".into(),
                fingerprint: "3f9c".into(),
                commit: Some("a1b2c3d".into()),
                built_ms: 1,
            }),
            ..HostStats::default()
        };
        let json = serde_json::to_string(&stats).expect("json");
        let back: HostStats = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, stats);
    }
}
