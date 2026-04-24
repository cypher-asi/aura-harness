use crate::protocol;
use crate::scheduler::Scheduler;
use aura_core::{
    resolve_effective_permission, AgentId, AgentToolPermissions, Identity,
    InstalledIntegrationDefinition, InstalledToolDefinition, RecordEntry, ToolState, Transaction,
    TransactionType, UserDefaultMode, UserToolDefaults,
};
use aura_reasoner::ToolDefinition;
use aura_store::Store;
use aura_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};
use bytes::Bytes;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct AgentToolContext {
    pub tool_permissions: Option<AgentToolPermissions>,
    pub originating_user_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct EffectiveToolInfo {
    pub name: String,
    pub description: String,
    pub effective_state: protocol::ToolStateWire,
}

pub(crate) fn validate_user_defaults(
    defaults: &UserToolDefaults,
    catalog: &ToolCatalog,
) -> Result<(), String> {
    if let UserDefaultMode::DefaultPermissions { per_tool, .. } = &defaults.mode {
        validate_tool_names(per_tool.keys().map(String::as_str), catalog)?;
    }
    Ok(())
}

pub(crate) fn validate_agent_tool_permissions(
    permissions: &AgentToolPermissions,
    catalog: &ToolCatalog,
) -> Result<(), String> {
    validate_tool_names(permissions.per_tool.keys().map(String::as_str), catalog)
}

fn validate_tool_names<'a>(
    names: impl Iterator<Item = &'a str>,
    catalog: &ToolCatalog,
) -> Result<(), String> {
    let known = catalog
        .tools_for_profile_with_permissions(ToolProfile::Agent, None)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<HashSet<_>>();
    for name in names {
        if !known.contains(name) {
            return Err(format!("unknown tool '{name}'"));
        }
    }
    Ok(())
}

pub(crate) fn effective_tool_definitions(
    catalog: &ToolCatalog,
    tool_config: &ToolConfig,
    installed_tools: &[InstalledToolDefinition],
    installed_integrations: &[InstalledIntegrationDefinition],
    user_default: &UserToolDefaults,
    agent_override: Option<&AgentToolPermissions>,
) -> Vec<(ToolDefinition, ToolState)> {
    let mut seen = HashSet::new();
    let mut tools = Vec::new();
    for tool in catalog.visible_tools(ToolProfile::Agent, tool_config) {
        let state = resolve_effective_permission(user_default, agent_override, &tool.name);
        if state != ToolState::Deny && seen.insert(tool.name.clone()) {
            tools.push((tool, state));
        }
    }
    for tool in installed_tools {
        if !tool_has_required_integration(
            installed_integrations,
            tool.required_integration.as_ref(),
        ) {
            continue;
        }
        let state = resolve_effective_permission(user_default, agent_override, &tool.name);
        if state != ToolState::Deny && seen.insert(tool.name.clone()) {
            tools.push((
                ToolDefinition::new(&tool.name, &tool.description, tool.input_schema.clone()),
                state,
            ));
        }
    }
    tools
}

pub(crate) fn effective_tool_infos(
    catalog: &ToolCatalog,
    tool_config: &ToolConfig,
    user_default: &UserToolDefaults,
    agent_override: Option<&AgentToolPermissions>,
) -> Vec<EffectiveToolInfo> {
    catalog
        .visible_tools(ToolProfile::Agent, tool_config)
        .into_iter()
        .filter_map(|tool| {
            let state = resolve_effective_permission(user_default, agent_override, &tool.name);
            (state != ToolState::Deny).then(|| EffectiveToolInfo {
                name: tool.name,
                description: tool.description,
                effective_state: protocol::tool_state_to_wire(state),
            })
        })
        .collect()
}

pub(crate) fn load_agent_tool_context(
    store: &Arc<dyn Store>,
    agent_id: AgentId,
) -> Result<AgentToolContext, String> {
    let head = store
        .get_head_seq(agent_id)
        .map_err(|e| format!("get_head_seq: {e}"))?;
    if head == 0 {
        return Ok(AgentToolContext {
            tool_permissions: None,
            originating_user_id: None,
        });
    }
    let from_seq = head.saturating_sub(255).max(1);
    let entries = store
        .scan_record(agent_id, from_seq, 256)
        .map_err(|e| format!("scan_record: {e}"))?;
    Ok(context_from_entries(entries))
}

fn context_from_entries(entries: Vec<RecordEntry>) -> AgentToolContext {
    let mut originating_user_id = None;
    let mut tool_permissions = None;
    for entry in entries {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&entry.tx.payload) else {
            continue;
        };
        if let Some(parsed) = value
            .get("identity")
            .and_then(|v| serde_json::from_value::<Identity>(v.clone()).ok())
        {
            tool_permissions = parsed.tool_permissions.clone();
        }
        if let Some(user_id) = value.get("originating_user_id").and_then(|v| v.as_str()) {
            originating_user_id = Some(user_id.to_string());
        }
        if value.get("kind").and_then(|v| v.as_str()) == Some("agent_tool_permissions") {
            tool_permissions = value
                .get("tool_permissions")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
        }
    }
    AgentToolContext {
        tool_permissions,
        originating_user_id,
    }
}

/// Append an `agent_tool_permissions` System entry to the agent's log.
///
/// Acquires the scheduler's per-agent lock (Invariant §12) before the
/// `append_entry_direct` call so this HTTP-driven write serializes with
/// the scheduler's inbox-drain on the same agent. Without this the
/// single-writer guarantee can be violated if a scheduler tick is
/// running concurrently.
pub(crate) async fn append_agent_tool_permissions_entry(
    store: &Arc<dyn Store>,
    scheduler: &Arc<Scheduler>,
    agent_id: AgentId,
    permissions: &AgentToolPermissions,
) -> Result<Transaction, String> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "kind": "agent_tool_permissions",
        "agent_id": agent_id,
        "tool_permissions": permissions,
    }))
    .map_err(|e| format!("serialize agent_tool_permissions: {e}"))?;
    let tx = Transaction::new_chained(
        agent_id,
        TransactionType::System,
        Bytes::from(payload),
        None,
    );

    // Hold the per-agent lock for the entire read-modify-write window so a
    // concurrent scheduler drain cannot wedge a different entry at the same
    // seq between our `get_head_seq` and `append_entry_direct`.
    let _guard = scheduler.agent_lock(agent_id).await;

    let head = store
        .get_head_seq(agent_id)
        .map_err(|e| format!("get_head_seq: {e}"))?;
    let from_seq = head.saturating_sub(49).max(1);
    let window = if head == 0 {
        Vec::new()
    } else {
        store
            .scan_record(agent_id, from_seq, 50)
            .map_err(|e| format!("scan_record: {e}"))?
    };
    let context_hash =
        aura_kernel::hash_tx_with_window(&tx, &window).map_err(|e| format!("context hash: {e}"))?;
    let seq = head + 1;
    let entry = RecordEntry::builder(seq, tx.clone())
        .context_hash(context_hash)
        .build();
    store
        .append_entry_direct(agent_id, seq, &entry)
        .map_err(|e| format!("append_entry_direct: {e}"))?;
    Ok(tx)
}

pub(crate) fn enforce_monotonic_update(
    user_default: &UserToolDefaults,
    current: Option<&AgentToolPermissions>,
    next: &AgentToolPermissions,
) -> Result<(), String> {
    for (tool, next_state) in &next.per_tool {
        let current_state = resolve_effective_permission(user_default, current, tool);
        if !next_state.is_subset_of(current_state) {
            return Err(format!(
                "tool '{tool}' cannot be widened from {} to {}",
                state_label(current_state),
                state_label(*next_state)
            ));
        }
    }
    Ok(())
}

fn tool_has_required_integration(
    installed_integrations: &[InstalledIntegrationDefinition],
    requirement: Option<&aura_core::InstalledToolIntegrationRequirement>,
) -> bool {
    let Some(requirement) = requirement else {
        return true;
    };
    installed_integrations.iter().any(|integration| {
        requirement
            .integration_id
            .as_deref()
            .map_or(true, |expected| integration.integration_id == expected)
            && requirement
                .provider
                .as_deref()
                .map_or(true, |expected| integration.provider == expected)
            && requirement
                .kind
                .as_deref()
                .map_or(true, |expected| integration.kind == expected)
    })
}

fn state_label(state: ToolState) -> &'static str {
    match state {
        ToolState::Allow => "on",
        ToolState::Deny => "off",
        ToolState::Ask => "ask",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn defaults(entries: &[(&str, ToolState)], fallback: ToolState) -> UserToolDefaults {
        UserToolDefaults::default_permissions(
            entries
                .iter()
                .map(|(tool, state)| ((*tool).to_string(), *state))
                .collect(),
            fallback,
        )
    }

    fn overrides(entries: &[(&str, ToolState)]) -> AgentToolPermissions {
        AgentToolPermissions {
            per_tool: entries
                .iter()
                .map(|(tool, state)| ((*tool).to_string(), *state))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn validate_user_defaults_rejects_unknown_tool_names() {
        let catalog = ToolCatalog::new();
        let unknown = defaults(&[("not_a_real_tool", ToolState::Allow)], ToolState::Deny);

        let err = validate_user_defaults(&unknown, &catalog).expect_err("unknown tool rejected");
        assert!(err.contains("unknown tool 'not_a_real_tool'"));
    }

    #[test]
    fn validate_agent_permissions_accepts_catalog_tool_names() {
        let catalog = ToolCatalog::new();
        let permissions = overrides(&[("read_file", ToolState::Ask)]);

        validate_agent_tool_permissions(&permissions, &catalog).expect("known tool accepted");
    }

    #[test]
    fn monotonic_update_rejects_widening_and_allows_narrowing() {
        let user_default = defaults(&[("run_command", ToolState::Ask)], ToolState::Allow);
        let current = overrides(&[("read_file", ToolState::Ask)]);

        let widening = overrides(&[
            ("read_file", ToolState::Allow),
            ("run_command", ToolState::Allow),
        ]);
        let err = enforce_monotonic_update(&user_default, Some(&current), &widening)
            .expect_err("widening should be rejected");
        assert!(err.contains("cannot be widened"));

        let narrowing = overrides(&[
            ("read_file", ToolState::Deny),
            ("run_command", ToolState::Deny),
        ]);
        enforce_monotonic_update(&user_default, Some(&current), &narrowing)
            .expect("narrowing should be accepted");
    }
}
