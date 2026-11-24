//! A host's vital facts: name, OS, addresses, CPU, memory and disk.
//!
//! The same collector serves this machine (the hub's HOST tab) and a
//! cloud node (`GET /v1/host`). A boxd machine is a microVM with its own
//! kernel, so the OS reports the machine's own limits and no cgroup
//! parsing is needed.
//!
//! [`HostSampler`] keeps the `sysinfo` state between samples, since CPU
//! usage is a difference between two refreshes. Sampling is blocking:
//! call it from `spawn_blocking`.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use aura_core_protocol::cloud::{HostAddress, HostStats, SnapshotOrigin};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

/// Keeps what is needed to sample a host repeatedly.
pub struct HostSampler {
    system: System,
    disk_path: PathBuf,
    manifest: Option<PathBuf>,
}

impl std::fmt::Debug for HostSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSampler")
            .field("disk_path", &self.disk_path)
            .finish_non_exhaustive()
    }
}

impl HostSampler {
    /// A sampler reporting disk figures for the filesystem holding
    /// `disk_path`, and the snapshot manifest at `manifest` if given.
    #[must_use]
    pub fn new(disk_path: impl Into<PathBuf>, manifest: Option<PathBuf>) -> Self {
        let refresh = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
            .with_memory(MemoryRefreshKind::nothing().with_ram());
        Self {
            system: System::new_with_specifics(refresh),
            disk_path: disk_path.into(),
            manifest,
        }
    }

    /// Take a sample. Blocking.
    pub fn sample(&mut self) -> HostStats {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let load = System::load_average();
        let (disk_used, disk_total) = disk_for(&self.disk_path);
        HostStats {
            hostname: System::host_name().unwrap_or_default(),
            os: System::long_os_version()
                .or_else(System::name)
                .unwrap_or_else(|| std::env::consts::OS.to_string()),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: std::env::consts::ARCH.to_string(),
            uptime_secs: System::uptime(),
            cpu_count: u32::try_from(self.system.cpus().len()).unwrap_or(u32::MAX),
            cpu_percent: self.system.global_cpu_usage(),
            load: [load.one, load.five, load.fifteen],
            cpu_busy_secs: cpu_busy_secs().unwrap_or(0.0),
            mem_used_bytes: self.system.used_memory(),
            mem_total_bytes: self.system.total_memory(),
            disk_used_bytes: disk_used,
            disk_total_bytes: disk_total,
            disk_path: self.disk_path.display().to_string(),
            addresses: addresses(),
            sampled_ms: u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
            snapshot: self.manifest.as_deref().and_then(read_manifest),
        }
    }
}

fn read_manifest(path: &Path) -> Option<SnapshotOrigin> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Used and total bytes of the filesystem whose mount point is the
/// longest prefix of `path`.
fn disk_for(path: &Path) -> (u64, u64) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map_or((0, 0), |disk| {
            let total = disk.total_space();
            (total.saturating_sub(disk.available_space()), total)
        })
}

/// Whether `ip` is private, loopback or link-local.
#[must_use]
pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn addresses() -> Vec<HostAddress> {
    let networks = Networks::new_with_refreshed_list();
    let mut out: Vec<HostAddress> = networks
        .iter()
        .flat_map(|(name, data)| {
            data.ip_networks().iter().map(move |net| HostAddress {
                interface: name.clone(),
                ip: net.addr.to_string(),
                private: is_private(&net.addr),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (a.private && a.ip.starts_with("127."), &a.interface, &a.ip).cmp(&(
            b.private && b.ip.starts_with("127."),
            &b.interface,
            &b.ip,
        ))
    });
    out
}

/// CPU-seconds busy since boot, summed over CPUs (Linux `/proc/stat`).
#[cfg(target_os = "linux")]
fn cpu_busy_secs() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    parse_proc_stat(&stat)
}

#[cfg(not(target_os = "linux"))]
fn cpu_busy_secs() -> Option<f64> {
    None
}

/// Busy CPU-seconds from `/proc/stat`'s aggregate `cpu` line: user,
/// nice, system, irq, softirq and steal, in USER_HZ (100 per second).
#[must_use]
pub fn parse_proc_stat(stat: &str) -> Option<f64> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|field| field.parse().ok())
        .collect();
    let at = |index: usize| fields.get(index).copied().unwrap_or(0);
    // user nice system idle iowait irq softirq steal
    let busy = at(0) + at(1) + at(2) + at(5) + at(6) + at(7);
    #[allow(clippy::cast_precision_loss)]
    let secs = busy as f64 / 100.0;
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_busy_time_skips_idle_and_iowait() {
        let stat = "cpu  100 20 30 5000 70 1 2 3 0 0\ncpu0 50 10 15 2500 35 0 1 1 0 0\n";
        assert_eq!(parse_proc_stat(stat), Some(1.56));
        assert_eq!(parse_proc_stat("intr 1"), None);
    }

    #[test]
    fn private_addresses() {
        for ip in [
            "10.42.0.17",
            "192.168.1.2",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "fd00::1",
            "fe80::1",
            "::1",
        ] {
            assert!(is_private(&ip.parse().expect("ip")), "{ip}");
        }
        for ip in ["34.120.7.9", "2001:db8::1"] {
            assert!(!is_private(&ip.parse().expect("ip")), "{ip}");
        }
    }

    #[test]
    fn a_sample_of_this_machine_is_filled_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = dir.path().join("snapshot.json");
        std::fs::write(
            &manifest,
            r#"{"name":"aura-proj-x","fingerprint":"3f9c","commit":"a1b2","built_ms":1}"#,
        )
        .expect("write");
        let mut sampler = HostSampler::new(dir.path(), Some(manifest));
        let stats = sampler.sample();
        assert!(stats.cpu_count > 0);
        assert!(stats.mem_total_bytes > 0);
        assert!(!stats.arch.is_empty());
        assert!(stats.sampled_ms > 0);
        assert_eq!(
            stats.snapshot.map(|s| s.name),
            Some("aura-proj-x".to_string())
        );
    }
}
