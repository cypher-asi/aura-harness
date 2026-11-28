//! Naming the agents another node holds the way agents name each other:
//! by their code (`RUS198`), their full name or their id.
//!
//! A node's directory lists each peer it reaches with the name the hub
//! gives it, `CODE (definition #n)` for a channel member. Agents are told
//! to address each other by code, so a target is matched against the
//! code, the name and the id, case-insensitively; only an exact, single
//! match counts. The directory itself is the boundary: only the agents
//! the hub lets this node reach are ever matched.

use aura_core_types::AgentId;

/// The code a directory entry goes by: its `code` field, else
/// [`code_in_name`]; `None` for a plain name.
pub(super) fn peer_code(peer: &serde_json::Value) -> Option<String> {
    if let Some(code) = peer["code"].as_str().filter(|code| !code.is_empty()) {
        return Some(code.to_string());
    }
    code_in_name(peer["name"].as_str()?).map(str::to_string)
}

/// The code in a name the hub gives a channel member, `CODE (name)`:
/// the part before ` (`, when it is one word of letters and digits.
#[must_use]
pub fn code_in_name(name: &str) -> Option<&str> {
    let (code, rest) = name.split_once(" (")?;
    let is_code =
        !code.is_empty() && rest.ends_with(')') && code.chars().all(|c| c.is_ascii_alphanumeric());
    is_code.then_some(code)
}

/// What `target` names among `peers`, directory entries in the shape
/// `list_agents` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Named {
    /// Exactly one peer
    One(AgentId),
    /// Several peers: `target` is ambiguous
    Several(Vec<String>),
    /// None
    None,
}

/// The peer of `peers` that `target` names by id, code or full name.
pub(super) fn find_peer(peers: &[serde_json::Value], target: &str) -> Named {
    let target = target.trim().trim_start_matches('@');
    if target.is_empty() {
        return Named::None;
    }
    let lower = target.to_ascii_lowercase();
    let matches: Vec<&serde_json::Value> = peers
        .iter()
        .filter(|peer| {
            peer["agent_id"].as_str() == Some(lower.as_str())
                || peer_code(peer).is_some_and(|code| code.eq_ignore_ascii_case(target))
                || peer["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(target))
        })
        .collect();
    let ids: Vec<AgentId> = matches
        .iter()
        .filter_map(|peer| peer["agent_id"].as_str())
        .filter_map(|hex| AgentId::from_hex(hex).ok())
        .collect();
    match ids.as_slice() {
        [] => Named::None,
        [one] => Named::One(*one),
        _ => Named::Several(matches.iter().map(|peer| label(peer)).collect()),
    }
}

/// `RUS198 (rust-engineer #198) a1b2c3d4`: how an error names a peer.
fn label(peer: &serde_json::Value) -> String {
    let name = peer["name"].as_str().unwrap_or("?");
    let id = peer["agent_id"].as_str().unwrap_or_default();
    format!("{name} {}", id.get(..8).unwrap_or(id))
}

/// What the agents the caller can reach go by, for an error: each one's
/// code, else its name.
pub(super) fn reachable(peers: &[serde_json::Value]) -> Vec<String> {
    let mut names: Vec<String> = peers
        .iter()
        .filter(|peer| peer["is_you"].as_bool() != Some(true))
        .filter_map(|peer| peer_code(peer).or_else(|| peer["name"].as_str().map(str::to_string)))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The error for a `target` that names no agent the caller reaches,
/// listing those it does.
pub(super) fn unknown(target: &str, reachable: &[String]) -> String {
    if reachable.is_empty() {
        return format!(
            "unknown agent '{target}'; call list_agents to see the agents you can reach"
        );
    }
    format!(
        "unknown agent '{target}'; you can reach: {}. Address them by code or agent id",
        reachable.join(", ")
    )
}

/// The error for a `target` several agents go by.
pub(super) fn ambiguous(target: &str, matches: &[String]) -> String {
    format!(
        "'{target}' matches several agents: {}; use the agent id",
        matches.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn peer(id: AgentId, name: &str) -> serde_json::Value {
        json!({"agent_id": id.to_hex(), "name": name, "node": "hub", "remote": true})
    }

    #[test]
    fn a_code_is_read_from_the_name_or_its_field() {
        let id = AgentId::new([1; 32]);
        assert_eq!(
            peer_code(&peer(id, "RUS198 (rust-engineer #198)")).as_deref(),
            Some("RUS198")
        );
        assert_eq!(peer_code(&peer(id, "cloud-scout")), None);
        assert_eq!(peer_code(&peer(id, "two words (x)")), None);
        let with_field = json!({"agent_id": id.to_hex(), "name": "x", "code": "CO2"});
        assert_eq!(peer_code(&with_field).as_deref(), Some("CO2"));
    }

    #[test]
    fn peers_are_found_by_code_name_or_id_in_any_case() {
        let (a, b) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
        let peers = [
            peer(a, "RUS198 (rust-engineer #198)"),
            peer(b, "FEN6 (frontend-engineer #6)"),
        ];
        assert_eq!(find_peer(&peers, "RUS198"), Named::One(a));
        assert_eq!(find_peer(&peers, "@rus198"), Named::One(a));
        assert_eq!(
            find_peer(&peers, "fen6 (frontend-engineer #6)"),
            Named::One(b)
        );
        assert_eq!(find_peer(&peers, &b.to_hex().to_uppercase()), Named::One(b));
        assert_eq!(find_peer(&peers, "RUS999"), Named::None);
        assert_eq!(find_peer(&peers, " "), Named::None);
    }

    #[test]
    fn a_code_two_peers_share_is_ambiguous() {
        let (a, b) = (AgentId::new([1; 32]), AgentId::new([2; 32]));
        let peers = [peer(a, "DUP1 (x #1)"), peer(b, "DUP1 (y #1)")];
        let Named::Several(found) = find_peer(&peers, "dup1") else {
            panic!("ambiguous");
        };
        assert_eq!(found.len(), 2);
        assert!(ambiguous("dup1", &found).contains("use the agent id"));
    }

    #[test]
    fn the_unknown_error_lists_whom_the_caller_reaches() {
        let a = AgentId::new([1; 32]);
        let peers = [
            peer(a, "RUS198 (rust-engineer #198)"),
            json!({"agent_id": "x", "name": "me", "is_you": true}),
            json!({"agent_id": "y", "name": "scout"}),
        ];
        let said = unknown("RUS999", &reachable(&peers));
        assert!(said.contains("unknown agent 'RUS999'"), "{said}");
        assert!(said.contains("RUS198, scout"), "{said}");
        assert!(!said.contains(", me") && !said.contains(": me"), "{said}");
        assert!(unknown("x", &[]).contains("call list_agents"));
    }
}
