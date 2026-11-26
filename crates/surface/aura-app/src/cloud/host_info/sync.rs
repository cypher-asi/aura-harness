//! The SYNC section: how current this computer's copy of a cloud agent is.

use aura_ui::host::{format_duration, HostFact, HostSection, HostView};

use crate::cloud::hub::SyncStatus;

/// Add how current this computer's copy of a cloud agent is to its view,
/// after its machine; a copy that is behind or failing is flagged.
pub fn add_sync(view: &mut HostView, sync: &SyncStatus, now_ms: u64) {
    let ago = |at: u64| format_duration(now_ms.saturating_sub(at) / 1000);
    let copied = |at: Option<u64>| {
        at.map_or_else(
            || "not copied yet".to_string(),
            |at| format!("copied {} ago", ago(at)),
        )
    };
    let mut record = match sync.node_seq {
        Some(node) if node > sync.copied_seq => {
            format!("{} of {node} entries here", sync.copied_seq)
        }
        _ => format!("{} entries here", sync.copied_seq),
    };
    if sync.copied_ms.is_some() {
        record.push_str(&format!(" · {}", copied(sync.copied_ms)));
    }
    if let Some(error) = &sync.error {
        record.push_str(&format!(" · {error}"));
    }
    let mut facts = vec![HostFact {
        warn: sync.behind(),
        ..HostFact::new("Record", record)
    }];
    if sync.children > 0 {
        facts.push(HostFact::new(
            "Spawned",
            format!("{} agents, copied with it", sync.children),
        ));
    }
    facts.push(HostFact::new("Session", copied(sync.session_ms)));
    facts.push(HostFact::new("Traces", copied(sync.traces_ms)));
    facts.push(HostFact::new(
        "Changes",
        sync.patch_ms.map_or_else(
            || "saved when the machine sleeps".to_string(),
            |at| format!("saved at its last sleep, {} ago", ago(at)),
        ),
    ));
    let at = view.sections.len().min(1);
    view.sections.insert(
        at,
        HostSection {
            title: "SYNC".into(),
            facts,
        },
    );
}
