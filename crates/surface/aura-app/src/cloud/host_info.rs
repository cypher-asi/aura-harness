//! Builds what the HOST tab shows: this machine for a local agent, the
//! agent's boxd machine for a cloud agent, and every host of the project
//! for the project scope.
//!
//! Local facts come from a [`HostSampler`] (blocking: sampled on
//! `spawn_blocking`); cloud facts from the fleet's machine view, the
//! node's last [`HostStats`] and the cost model. A sleeping machine is
//! never woken to fill the tab: its last known facts are shown with
//! their time.

use aura_core_protocol::cloud::HostStats;
use aura_core_types::{AgentId, VmPayload};
use aura_engine_cloud::cost::CostEstimate;
use aura_engine_cloud::fleet::MachineView;
use aura_ui_terminal::host::{
    format_bytes, format_duration, format_money, HostAction, HostFact, HostRow, HostSection,
    HostView, PlacementView,
};

/// Facts every host has, from one sample.
fn resource_sections(stats: &HostStats) -> Vec<HostSection> {
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
    let mut sections = vec![machine, resources];
    if !network.facts.is_empty() {
        sections.push(network);
    }
    sections
}

/// The view of this machine, for a local agent.
#[must_use]
pub fn local_view(
    agent: AgentId,
    stats: &HostStats,
    cloud_ready: bool,
    key_file: Option<&str>,
) -> HostView {
    let mut sections = resource_sections(stats);
    let cloud = match (cloud_ready, key_file) {
        (true, Some(file)) => format!("set up (key from {file})"),
        (true, None) => "set up".to_string(),
        (false, _) => "not set up · run aura cloud setup or add BOXD_API_KEY to .env".to_string(),
    };
    sections.push(HostSection {
        title: "CLOUD".into(),
        facts: vec![HostFact::new("boxd", cloud)],
    });
    HostView {
        agent_id: agent.to_hex(),
        placement: PlacementView::Local,
        title: format!("this machine · {}", stats.hostname),
        status: format!("● running · up {}", format_duration(stats.uptime_secs)),
        sections,
        actions: if cloud_ready {
            vec![HostAction::Flip]
        } else {
            Vec::new()
        },
        project: Vec::new(),
        project_total: None,
        as_of: None,
    }
}

fn hms(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// The view of a cloud agent's machine.
#[must_use]
pub fn cloud_view(
    agent: AgentId,
    view: &MachineView,
    stats: Option<&HostStats>,
    cost: Option<&CostEstimate>,
    rows: &[VmPayload],
) -> HostView {
    let machine = &view.machine;
    let mut status = format!("● {}", view.state.label());
    if let Some(stats) = stats {
        status.push_str(&format!(" · up {}", format_duration(stats.uptime_secs)));
    }
    if let Some(secs) = view.sleeps_in_secs.filter(|_| view.busy == 0) {
        status.push_str(&format!(" · sleeps in {}", format_duration(secs)));
    }
    let mut sections = vec![HostSection {
        title: "MACHINE".into(),
        facts: vec![
            HostFact::new("Provider", "boxd"),
            HostFact::new("Name", machine.name.clone()),
            HostFact::new(
                "Region",
                if machine.region.is_empty() {
                    "fra".into()
                } else {
                    machine.region.clone()
                },
            ),
            HostFact::new(
                "Size",
                format!(
                    "{} vCPU · {} · 100 GiB disk",
                    machine.vcpu,
                    format_bytes(machine.memory_bytes)
                ),
            ),
            HostFact::new(
                "Isolated",
                if machine.isolated {
                    "yes"
                } else {
                    "no (private network)"
                },
            ),
            HostFact::new("Domain", machine.url()),
        ],
    }];
    if !machine.public_ip.is_empty() {
        sections[0]
            .facts
            .push(HostFact::new("Proxy IP", machine.public_ip.clone()));
    }
    if let Some(source) = &machine.source {
        let from = match source {
            aura_core_types::VmSource::Snapshot { name, .. } => format!("snapshot {name}"),
            aura_core_types::VmSource::Fork { parent } => format!("fork of {parent}"),
            aura_core_types::VmSource::Image { image } if image.is_empty() => {
                "boxd default image".to_string()
            }
            aura_core_types::VmSource::Image { image } => format!("image {image}"),
        };
        sections[0].facts.push(HostFact::new("From", from));
    }
    if let Some(stats) = stats {
        let mut more = resource_sections(stats);
        // The node's own machine facts repeat the name; keep its OS.
        if let Some(first) = more.first_mut() {
            first.facts.retain(|fact| fact.label != "Host");
            sections[0].facts.append(&mut first.facts);
        }
        sections.extend(more.into_iter().skip(1));
        if let Some(origin) = &stats.snapshot {
            sections[0].facts.push(HostFact::new(
                "Snapshot",
                format!(
                    "{} · {}{}",
                    origin.name,
                    &origin.fingerprint[..origin.fingerprint.len().min(8)],
                    origin
                        .commit
                        .as_deref()
                        .map(|c| format!(" · {}", &c[..c.len().min(7)]))
                        .unwrap_or_default()
                ),
            ));
        }
    }
    if let Some(cost) = cost {
        let mut facts = vec![
            HostFact::new(
                "Now",
                format!("{}/h est.", format_money(cost.per_hour, &cost.currency)),
            ),
            HostFact::new(
                "So far",
                format!("{} ", format_money(cost.total(), &cost.currency))
                    + if cost.estimated > 0.0 {
                        "(metered + est.)"
                    } else {
                        "(metered)"
                    },
            ),
        ];
        if cost.metered_until_ms > 0 {
            facts.push(HostFact::new(
                "Metered",
                format!(
                    "{} through {}",
                    format_money(cost.metered, &cost.currency),
                    hms(cost.metered_until_ms)
                ),
            ));
        } else if view.usage_denied {
            facts.push(HostFact::new(
                "Metered",
                "needs an org admin key; showing estimates",
            ));
        }
        sections.push(HostSection {
            title: "COST".into(),
            facts,
        });
    }
    if !rows.is_empty() {
        sections.push(HostSection {
            title: "EVENTS".into(),
            facts: rows
                .iter()
                .rev()
                .take(8)
                .map(|row| {
                    HostFact::new(
                        row.event.as_str(),
                        row.reason
                            .clone()
                            .or_else(|| row.status.clone())
                            .unwrap_or_else(|| row.machine_name.clone()),
                    )
                })
                .collect(),
        });
    }
    let mut actions = vec![HostAction::Pull];
    actions.push(if view.state.is_asleep() {
        HostAction::Wake
    } else {
        HostAction::Sleep
    });
    actions.push(HostAction::Flip);
    HostView {
        agent_id: agent.to_hex(),
        placement: PlacementView::Cloud,
        title: format!("boxd · {}", machine.name),
        status,
        sections,
        actions,
        project: Vec::new(),
        project_total: None,
        as_of: view
            .state
            .is_asleep()
            .then(|| hms(stats.map_or(view.seen_ms, |s| s.sampled_ms))),
    }
}

/// One project row for a cloud agent.
#[must_use]
pub fn cloud_row(
    agent: AgentId,
    name: &str,
    view: &MachineView,
    cost: Option<&CostEstimate>,
) -> HostRow {
    HostRow {
        agent_id: agent.to_hex(),
        agent: name.to_string(),
        host: view.machine.name.clone(),
        state: view.state.label().to_string(),
        per_hour: cost
            .map(|c| format!("{}/h", format_money(c.per_hour, &c.currency)))
            .unwrap_or_default(),
        total: cost
            .map(|c| format_money(c.total(), &c.currency))
            .unwrap_or_default(),
    }
}

/// One project row for a local agent.
#[must_use]
pub fn local_row(agent: AgentId, name: &str, running: bool) -> HostRow {
    HostRow {
        agent_id: agent.to_hex(),
        agent: name.to_string(),
        host: "this machine".into(),
        state: if running { "busy" } else { "ready" }.into(),
        per_hour: String::new(),
        total: String::new(),
    }
}

/// The project footer line over `costs`.
#[must_use]
pub fn project_total(costs: &[CostEstimate]) -> Option<String> {
    let first = costs.first()?;
    let per_hour: f64 = costs.iter().map(|c| c.per_hour).sum();
    let total: f64 = costs.iter().map(CostEstimate::total).sum();
    Some(format!(
        "{} cloud machine{} · {}/h est. · {} so far",
        costs.len(),
        if costs.len() == 1 { "" } else { "s" },
        format_money(per_hour, &first.currency),
        format_money(total, &first.currency)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_protocol::cloud::HostAddress;
    use aura_engine_cloud::fleet::VmState;
    use aura_engine_cloud::{MachineInfo, MachineStatus};
    use std::collections::BTreeMap;

    fn stats() -> HostStats {
        HostStats {
            hostname: "aura-re36-3f9c".into(),
            os: "Ubuntu 24.04".into(),
            arch: "x86_64".into(),
            uptime_secs: 8040,
            cpu_count: 4,
            cpu_percent: 37.0,
            mem_used_bytes: 6 << 30,
            mem_total_bytes: 16 << 30,
            disk_used_bytes: 20 << 30,
            disk_total_bytes: 100 << 30,
            disk_path: "/workspace".into(),
            addresses: vec![HostAddress {
                interface: "eth0".into(),
                ip: "10.42.0.17".into(),
                private: true,
            }],
            sampled_ms: 1,
            ..HostStats::default()
        }
    }

    fn machine_view(state: VmState) -> MachineView {
        MachineView {
            agent: "aa".into(),
            project: "p1".into(),
            ledger: "0://cloud/vm/x".into(),
            machine: MachineInfo {
                id: "id".into(),
                name: "aura-re36-3f9c".into(),
                status: MachineStatus::Running,
                region: String::new(),
                vcpu: 4,
                memory_bytes: 16 << 30,
                disk_bytes: 100 << 30,
                domain: "aura-re36-3f9c.boxd.sh".into(),
                public_ip: String::new(),
                ssh_port: 0,
                isolated: true,
                labels: BTreeMap::new(),
                source: Some(aura_core_types::VmSource::Snapshot {
                    name: "aura-proj-p1".into(),
                    version: None,
                }),
                created_ms: 0,
                boot_ms: 6,
            },
            state,
            busy: 0,
            idle_secs: 60,
            sleeps_in_secs: Some(240),
            usage: None,
            usage_denied: false,
            seen_ms: 1,
        }
    }

    fn cost() -> CostEstimate {
        CostEstimate {
            metered: 0.10,
            metered_until_ms: 1,
            complete: false,
            estimated: 0.02,
            per_hour: 0.05,
            currency: "eur".into(),
        }
    }

    fn facts(view: &HostView, section: &str) -> Vec<String> {
        view.sections
            .iter()
            .find(|s| s.title == section)
            .map(|s| {
                s.facts
                    .iter()
                    .map(|f| format!("{}: {}", f.label, f.value))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_local_view_has_resources_and_no_cost() {
        let view = local_view(AgentId::new([1; 32]), &stats(), false, None);
        assert_eq!(view.placement, PlacementView::Local);
        assert!(facts(&view, "RESOURCES")
            .iter()
            .any(|f| f.starts_with("Memory: 6.0 GiB of 16.0 GiB")));
        assert!(facts(&view, "NETWORK")
            .iter()
            .any(|f| f.contains("10.42.0.17 (private)")));
        assert!(facts(&view, "COST").is_empty());
        assert!(facts(&view, "CLOUD")[0].contains("not set up"));
        assert!(view.actions.is_empty());
    }

    #[test]
    fn a_cloud_view_has_its_machine_and_cost() {
        let view = cloud_view(
            AgentId::new([1; 32]),
            &machine_view(VmState::Ready),
            Some(&stats()),
            Some(&cost()),
            &[],
        );
        assert_eq!(view.title, "boxd · aura-re36-3f9c");
        assert!(
            view.status.contains("ready") && view.status.contains("sleeps in 4m"),
            "{}",
            view.status
        );
        assert!(facts(&view, "MACHINE")
            .iter()
            .any(|f| f == "Size: 4 vCPU · 16.0 GiB · 100 GiB disk"));
        assert!(
            facts(&view, "COST")
                .iter()
                .any(|f| f == "Now: €0.0500/h est."),
            "{:?}",
            facts(&view, "COST")
        );
        assert!(view.actions.contains(&HostAction::Sleep));
        assert!(view.as_of.is_none());
    }

    #[test]
    fn a_sleeping_machine_shows_its_last_facts_and_offers_wake() {
        let view = cloud_view(
            AgentId::new([1; 32]),
            &machine_view(VmState::Hibernated),
            Some(&stats()),
            None,
            &[],
        );
        assert!(view.actions.contains(&HostAction::Wake));
        assert!(view.as_of.is_some());
    }

    #[test]
    fn project_totals_add_up() {
        let line = project_total(&[cost(), cost()]).expect("line");
        assert_eq!(line, "2 cloud machines · €0.10/h est. · €0.24 so far");
        assert!(project_total(&[]).is_none());
    }
}
