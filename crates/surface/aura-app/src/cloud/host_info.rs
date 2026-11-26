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
use aura_engine_cloud::github::RepoRef;
use aura_engine_cloud::plans::PlanStatus;
use aura_engine_cloud::repos::RepoStatus;
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

/// Add who placed a cloud agent, and the state of their plans its
/// machine can use, to its view.
pub fn add_owner_and_models(view: &mut HostView, owner: &str, plans: &[PlanStatus], now_ms: u64) {
    if let Some(machine) = view.sections.first_mut() {
        machine
            .facts
            .insert(1, HostFact::new("Owner", owner.to_string()));
    }
    if plans.is_empty() {
        return;
    }
    let facts = plans
        .iter()
        .map(|plan| {
            let mut value = if plan.linked {
                match plan.synced_ms {
                    Some(at) => format!(
                        "linked · token synced {} ago",
                        format_duration(now_ms.saturating_sub(at) / 1000)
                    ),
                    None => "linked".to_string(),
                }
            } else {
                format!("not linked · /login {}", plan.kind.id())
            };
            value.push_str(&format!(" · {}", plan.host));
            let fact = HostFact::new(plan.kind.label(), value);
            match &plan.error {
                Some(error) if plan.linked => HostFact {
                    value: format!("{} · {error}", fact.value),
                    warn: true,
                    ..fact
                },
                _ => fact,
            }
        })
        .collect();
    let at = view.sections.len().min(1);
    view.sections.insert(
        at,
        HostSection {
            title: "MODELS".into(),
            facts,
        },
    );
}

/// What a cloud agent's view shows of its git.
#[derive(Debug, Clone, Copy)]
pub struct GitFacts<'a> {
    /// Its branch.
    pub branch: &'a str,
    /// The GitHub repository it pushes to.
    pub repo: Option<&'a RepoRef>,
    /// That repository's token on its machine; `None` until synced.
    pub token: Option<&'a RepoStatus>,
    /// The branch's tip last fetched here.
    pub fetched: Option<&'a str>,
}

/// Add a cloud agent's branch, and the GitHub access its machine has,
/// to its view, after its machine.
pub fn add_git(view: &mut HostView, git: &GitFacts<'_>, now_ms: u64) {
    let mut facts = vec![HostFact::new("Branch", git.branch.to_string())];
    facts.push(HostFact::new(
        "Fetched here",
        git.fetched.map_or_else(
            || "not yet".to_string(),
            |tip| tip.chars().take(12).collect(),
        ),
    ));
    match git.repo {
        None => facts.push(HostFact::new(
            "GitHub",
            "no GitHub origin · commits come back with pulls",
        )),
        Some(repo) => {
            facts.push(HostFact::new("Repository", repo.full_name()));
            facts.push(match git.token {
                Some(status) if status.linked => HostFact::new(
                    "Token",
                    match status.synced_ms {
                        Some(at) => format!(
                            "synced {} ago · pushes as the App's bot",
                            format_duration(now_ms.saturating_sub(at) / 1000)
                        ),
                        None => "set · pushes as the App's bot".to_string(),
                    },
                ),
                Some(status) => HostFact {
                    warn: true,
                    ..HostFact::new(
                        "Token",
                        match (&status.install_url, &status.error) {
                            (Some(url), _) => format!("App not installed · {url}"),
                            (None, Some(error)) => format!("none · {error}"),
                            (None, None) => "none".to_string(),
                        },
                    )
                },
                None => HostFact::new("Token", "not synced since aura started"),
            });
        }
    }
    let at = view.sections.len().min(1);
    view.sections.insert(
        at,
        HostSection {
            title: "GIT".into(),
            facts,
        },
    );
}

/// One project row for a cloud agent; `owner` names the user it belongs
/// to when the project has several.
#[must_use]
pub fn cloud_row(
    agent: AgentId,
    name: &str,
    view: &MachineView,
    cost: Option<&CostEstimate>,
    owner: Option<String>,
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
        owner,
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
        owner: None,
    }
}

/// The project footer line over `costs` by owner: the total, then each
/// user's share when there are several.
#[must_use]
pub fn project_total_by_owner(costs: &[(String, CostEstimate)]) -> Option<String> {
    let all: Vec<CostEstimate> = costs.iter().map(|(_, cost)| cost.clone()).collect();
    let mut line = project_total(&all)?;
    let mut owners: Vec<&str> = Vec::new();
    for (owner, _) in costs {
        if !owners.contains(&owner.as_str()) {
            owners.push(owner);
        }
    }
    if owners.len() > 1 {
        for owner in owners {
            let mine: Vec<&CostEstimate> = costs
                .iter()
                .filter(|(o, _)| o == owner)
                .map(|(_, c)| c)
                .collect();
            let per_hour: f64 = mine.iter().map(|c| c.per_hour).sum();
            let currency = mine.first().map_or("eur", |c| c.currency.as_str());
            line.push_str(&format!(
                " · {owner} {}/h",
                format_money(per_hour, currency)
            ));
        }
    }
    Some(line)
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
            owner: "a11ce000".into(),
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
        let one = project_total_by_owner(&[("Alice".into(), cost()), ("Alice".into(), cost())])
            .expect("line");
        assert_eq!(one, line, "one owner: no split");
        let two = project_total_by_owner(&[("Alice".into(), cost()), ("Bob".into(), cost())])
            .expect("line");
        assert!(two.ends_with(" · Alice €0.0500/h · Bob €0.0500/h"), "{two}");
    }

    #[test]
    fn cloud_views_name_their_owner_and_plans() {
        use aura_core_auth::ProviderKind;
        let mut view = cloud_view(
            AgentId::new([1; 32]),
            &machine_view(VmState::Ready),
            None,
            None,
            &[],
        );
        let plans = [
            PlanStatus {
                kind: ProviderKind::Claude,
                linked: true,
                synced_ms: Some(1_000),
                host: "api.anthropic.com",
                error: None,
            },
            PlanStatus {
                kind: ProviderKind::Chatgpt,
                linked: false,
                synced_ms: Some(1_000),
                host: "api.openai.com",
                error: None,
            },
        ];
        add_owner_and_models(&mut view, "Alice", &plans, 181_000);
        assert!(
            facts(&view, "MACHINE").iter().any(|f| f == "Owner: Alice"),
            "{:?}",
            facts(&view, "MACHINE")
        );
        let models = facts(&view, "MODELS");
        assert_eq!(
            models,
            vec![
                "Claude: linked · token synced 3m ago · api.anthropic.com".to_string(),
                "ChatGPT: not linked · /login chatgpt · api.openai.com".to_string(),
            ]
        );
        assert_eq!(view.sections[1].title, "MODELS");
    }

    #[test]
    fn cloud_views_show_the_branch_and_why_github_is_out_of_reach() {
        let repo = RepoRef {
            owner: "cypher-asi".into(),
            name: "aura".into(),
        };
        let unlinked = RepoStatus {
            repo: repo.clone(),
            secret: "AURA_GH_ABC123_0123ABCD".into(),
            linked: false,
            expires_at_ms: None,
            synced_ms: Some(1_000),
            error: Some("not installed".into()),
            install_url: Some("https://github.com/apps/aura-agents/installations/new".into()),
        };
        let mut view = cloud_view(
            AgentId::new([1; 32]),
            &machine_view(VmState::Ready),
            None,
            None,
            &[],
        );
        add_git(
            &mut view,
            &GitFacts {
                branch: "aura/scout-a1b2",
                repo: Some(&repo),
                token: Some(&unlinked),
                fetched: None,
            },
            61_000,
        );
        assert_eq!(view.sections[1].title, "GIT");
        assert_eq!(
            facts(&view, "GIT"),
            vec![
                "Branch: aura/scout-a1b2".to_string(),
                "Fetched here: not yet".to_string(),
                "Repository: cypher-asi/aura".to_string(),
                "Token: App not installed · https://github.com/apps/aura-agents/installations/new"
                    .to_string(),
            ]
        );
        assert!(view.sections[1].facts[3].warn);

        let linked = RepoStatus {
            linked: true,
            error: None,
            install_url: None,
            ..unlinked
        };
        let mut view = cloud_view(
            AgentId::new([1; 32]),
            &machine_view(VmState::Ready),
            None,
            None,
            &[],
        );
        add_git(
            &mut view,
            &GitFacts {
                branch: "aura/scout-a1b2",
                repo: Some(&repo),
                token: Some(&linked),
                fetched: Some("0123456789abcdef0123456789abcdef01234567"),
            },
            61_000,
        );
        assert_eq!(
            facts(&view, "GIT")[1..],
            [
                "Fetched here: 0123456789ab".to_string(),
                "Repository: cypher-asi/aura".to_string(),
                "Token: synced 1m ago · pushes as the App's bot".to_string(),
            ]
        );
    }
}
