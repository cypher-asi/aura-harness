//! The resource sections every host has, from one [`HostStats`] sample.

use aura_core_protocol::cloud::HostStats;
use aura_ui::host::{format_bytes, format_duration, HostFact, HostSection};

/// Facts every host has, from one sample.
pub(super) fn resource_sections(stats: &HostStats) -> Vec<HostSection> {
    let machine = machine_section(stats);
    let resources = resources_section(stats);
    let network = network_section(stats);
    let mut sections = vec![machine, resources];
    if !network.facts.is_empty() {
        sections.push(network);
    }
    sections
}

fn machine_section(stats: &HostStats) -> HostSection {
    let mut machine = HostSection {
        title: "MACHINE".into(),
        facts: vec![
            HostFact::new("Host", stats.hostname.clone()),
            HostFact::new("OS", format!("{} ({})", stats.os, stats.arch)),
        ],
    };
    if !stats.kernel.is_empty() {
        machine
            .facts
            .push(HostFact::new("Kernel", stats.kernel.clone()));
    }
    machine
        .facts
        .push(HostFact::new("Uptime", format_duration(stats.uptime_secs)));
    machine
}

fn resources_section(stats: &HostStats) -> HostSection {
    let mem_percent = stats.mem_share() * 100.0;
    let disk_percent = stats.disk_share() * 100.0;
    let resources = HostSection {
        title: "RESOURCES".into(),
        facts: vec![
            HostFact::new(
                "CPU",
                format!(
                    "{} cores · {:.0}% · load {:.2} {:.2} {:.2}",
                    stats.cpu_count, stats.cpu_percent, stats.load[0], stats.load[1], stats.load[2]
                ),
            )
            .with_percent(f64::from(stats.cpu_percent)),
            HostFact::new(
                "Memory",
                format!(
                    "{} of {}",
                    format_bytes(stats.mem_used_bytes),
                    format_bytes(stats.mem_total_bytes)
                ),
            )
            .with_percent(mem_percent),
            HostFact::new(
                "Disk",
                format!(
                    "{} of {} ({})",
                    format_bytes(stats.disk_used_bytes),
                    format_bytes(stats.disk_total_bytes),
                    stats.disk_path
                ),
            )
            .with_percent(disk_percent),
        ],
    };
    resources
}

fn network_section(stats: &HostStats) -> HostSection {
    let mut network = HostSection {
        title: "NETWORK".into(),
        facts: Vec::new(),
    };
    for address in stats
        .addresses
        .iter()
        .filter(|a| !a.ip.starts_with("127.") && a.ip != "::1")
    {
        let kind = if address.private { "private" } else { "public" };
        network.facts.push(HostFact::new(
            address.interface.clone(),
            format!("{} ({kind})", address.ip),
        ));
    }
    network
}
