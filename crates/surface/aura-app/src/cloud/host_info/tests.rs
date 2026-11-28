use super::*;
use crate::cloud::hub::SyncStatus;
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
            last_connected_ms: 0,
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
fn the_sync_section_flags_an_agent_that_is_behind() {
    let mut view = cloud_view(
        AgentId::new([1; 32]),
        &machine_view(VmState::Ready),
        None,
        None,
        &[],
    );
    let current = SyncStatus {
        copied_seq: 40,
        copied_ms: Some(1_000),
        node_seq: Some(40),
        children: 2,
        session_ms: Some(61_000),
        ..SyncStatus::default()
    };
    add_sync(&mut view, &current, 121_000);
    assert_eq!(view.sections[1].title, "SYNC", "right after MACHINE");
    assert_eq!(
        facts(&view, "SYNC"),
        vec![
            "Record: 40 entries here · copied 2m ago",
            "Spawned: 2 agents, copied with it",
            "Session: copied 1m ago",
            "Traces: not copied yet",
            "Changes: saved when the machine sleeps",
        ]
    );
    assert!(!view.sections[1].facts[0].warn);

    let behind = SyncStatus {
        node_seq: Some(52),
        error: Some("node unreachable".into()),
        ..current
    };
    let mut view = cloud_view(
        AgentId::new([1; 32]),
        &machine_view(VmState::Ready),
        None,
        None,
        &[],
    );
    add_sync(&mut view, &behind, 121_000);
    let record = &view.sections[1].facts[0];
    assert!(record.warn, "a copy that is behind is flagged");
    assert_eq!(
        record.value,
        "40 of 52 entries here · copied 2m ago · node unreachable"
    );
}

#[test]
fn project_totals_add_up() {
    let line = project_total(&[cost(), cost()]).expect("line");
    assert_eq!(line, "2 cloud machines · €0.10/h est. · €0.24 so far");
    assert!(project_total(&[]).is_none());
    let one = project_total_by_owner(&[("Alice".into(), cost()), ("Alice".into(), cost())])
        .expect("line");
    assert_eq!(one, line, "one owner: no split");
    let two =
        project_total_by_owner(&[("Alice".into(), cost()), ("Bob".into(), cost())]).expect("line");
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

fn repo() -> RepoRef {
    RepoRef {
        owner: "cypher-asi".into(),
        name: "aura".into(),
    }
}

fn unlinked_repo() -> RepoStatus {
    RepoStatus {
        repo: repo(),
        secret: "AURA_GH_ABC123_0123ABCD".into(),
        linked: false,
        expires_at_ms: None,
        synced_ms: Some(1_000),
        error: Some("not installed".into()),
        install_url: Some("https://github.com/apps/aura-agents/installations/new".into()),
    }
}

/// A ready cloud agent's view with its git facts added.
fn git_view(token: &RepoStatus, fetched: Option<&str>) -> HostView {
    let repo = repo();
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
            token: Some(token),
            fetched,
        },
        61_000,
    );
    view
}

#[test]
fn cloud_views_show_the_branch_and_why_github_is_out_of_reach() {
    let view = git_view(&unlinked_repo(), None);
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
}

#[test]
fn cloud_views_show_the_fetched_tip_and_a_linked_token() {
    let linked = RepoStatus {
        linked: true,
        error: None,
        install_url: None,
        ..unlinked_repo()
    };
    let view = git_view(&linked, Some("0123456789abcdef0123456789abcdef01234567"));
    assert_eq!(
        facts(&view, "GIT")[1..],
        [
            "Fetched here: 0123456789ab".to_string(),
            "Repository: cypher-asi/aura".to_string(),
            "Token: synced 1m ago · pushes as the App's bot".to_string(),
        ]
    );
}
