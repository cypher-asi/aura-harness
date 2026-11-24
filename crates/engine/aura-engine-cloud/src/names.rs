//! Names of machines, ledger agents, snapshots and labels.
//!
//! Machine names become DNS labels (`<name>.boxd.sh`), so they are
//! lowercase ASCII letters, digits and `-`, at most 63 characters, and
//! never start or end with `-`.

use aura_core_types::AgentId;

/// Label every machine of this install carries, with the install id.
pub const LABEL_INSTALL: &str = "aura.install";
/// Label naming the agent a machine runs (hex id).
pub const LABEL_AGENT: &str = "aura.agent";
/// Label naming the project a machine works on.
pub const LABEL_PROJECT: &str = "aura.project";
/// Label naming what a machine is for: `agent` or `builder`.
pub const LABEL_ROLE: &str = "aura.role";
/// [`LABEL_ROLE`] of agent machines.
pub const ROLE_AGENT: &str = "agent";
/// [`LABEL_ROLE`] of snapshot builders.
pub const ROLE_BUILDER: &str = "builder";

/// Name of the base snapshot (toolchains and the aura binary).
pub const BASE_SNAPSHOT: &str = "aura-base";

/// Longest DNS label.
const MAX_LABEL: usize = 63;

/// `raw` as a DNS label fragment: lowercase, runs of anything else
/// turned into one `-`, trimmed of `-`.
#[must_use]
pub fn dns_fragment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn label(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .map(|part| dns_fragment(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let mut cut: String = joined.chars().take(MAX_LABEL).collect();
    while cut.ends_with('-') {
        cut.pop();
    }
    cut
}

/// Machine name for an agent: `aura-<code>-<4 hex of its id>`, e.g.
/// `aura-re36-3f9c`. Without a code, `aura-agent-<4 hex>`.
#[must_use]
pub fn machine_name(code: Option<&str>, agent_id: &AgentId) -> String {
    let hex = agent_id.to_hex();
    let short = &hex[..4.min(hex.len())];
    let code = code
        .map(dns_fragment)
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| "agent".to_string());
    label(&["aura", &code, short])
}

/// Name of a snapshot builder machine.
#[must_use]
pub fn builder_name(snapshot: &str, nonce: &str) -> String {
    label(&[snapshot, "build", nonce])
}

/// Snapshot name for a project: `aura-proj-<project id>`.
#[must_use]
pub fn project_snapshot(project_id: &str) -> String {
    let id: String = dns_fragment(project_id).chars().take(24).collect();
    label(&["aura", "proj", &id])
}

/// ZNS id of the ledger agent holding a cloud agent's machine rows.
#[must_use]
pub fn machine_ledger(agent_zns: &str) -> String {
    format!("0://cloud/vm/{}", agent_zns.trim_start_matches("0://"))
}

/// ZNS id of the ledger agent holding a project's snapshot builds.
#[must_use]
pub fn project_ledger(project_id: &str) -> String {
    format!("0://cloud/project/{project_id}")
}

/// Label selector matching every machine of `install`.
#[must_use]
pub fn install_selector(install: &str) -> String {
    format!("{LABEL_INSTALL}={install}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_names_are_dns_labels() {
        let id = AgentId::new([0x3f; 32]);
        assert_eq!(machine_name(Some("RE36"), &id), "aura-re36-3f3f");
        assert_eq!(machine_name(None, &id), "aura-agent-3f3f");
        assert_eq!(machine_name(Some("--"), &id), "aura-agent-3f3f");
        let long = "x".repeat(100);
        let name = machine_name(Some(&long), &id);
        assert!(name.len() <= 63, "{name}");
        assert!(!name.ends_with('-'));
    }

    #[test]
    fn fragments_collapse_odd_characters() {
        assert_eq!(dns_fragment("My Project!! v2"), "my-project-v2");
        assert_eq!(dns_fragment("ÄÖ"), "");
    }

    #[test]
    fn ledger_and_snapshot_names() {
        assert_eq!(
            machine_ledger("0://terminal/re36"),
            "0://cloud/vm/terminal/re36"
        );
        assert_eq!(project_ledger("p1"), "0://cloud/project/p1");
        assert_eq!(project_snapshot("01HX_ABC"), "aura-proj-01hx-abc");
        assert_eq!(install_selector("i1"), "aura.install=i1");
        assert_eq!(builder_name("aura-base", "ab12"), "aura-base-build-ab12");
    }
}
