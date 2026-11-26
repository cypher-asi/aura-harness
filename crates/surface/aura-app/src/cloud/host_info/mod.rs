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
use aura_ui::host::{
    format_bytes, format_duration, format_money, HostAction, HostFact, HostRow, HostSection,
    HostView, PlacementView,
};

mod sections;
mod sync;

use sections::resource_sections;
pub use sync::add_sync;

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
    let status = cloud_status(view, stats);
    let mut sections = vec![cloud_machine_section(machine)];
    if let Some(stats) = stats {
        add_node_stats(&mut sections, stats);
    }
    if let Some(cost) = cost {
        sections.push(cost_section(view, cost));
    }
    if !rows.is_empty() {
        sections.push(events_section(rows));
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

fn cloud_status(view: &MachineView, stats: Option<&HostStats>) -> String {
    let mut status = format!("● {}", view.state.label());
    if let Some(stats) = stats {
        status.push_str(&format!(" · up {}", format_duration(stats.uptime_secs)));
    }
    if let Some(secs) = view.sleeps_in_secs.filter(|_| view.busy == 0) {
        status.push_str(&format!(" · sleeps in {}", format_duration(secs)));
    }
    status
}

fn cloud_machine_section(machine: &aura_engine_cloud::MachineInfo) -> HostSection {
    let mut section = HostSection {
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
    };
    if !machine.public_ip.is_empty() {
        section
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
        section.facts.push(HostFact::new("From", from));
    }
    section
}

/// Merge the node's own sample into `sections`, whose first is the
/// machine section.
fn add_node_stats(sections: &mut Vec<HostSection>, stats: &HostStats) {
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

fn cost_section(view: &MachineView, cost: &CostEstimate) -> HostSection {
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
    HostSection {
        title: "COST".into(),
        facts,
    }
}

fn events_section(rows: &[VmPayload]) -> HostSection {
    HostSection {
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
mod tests;
