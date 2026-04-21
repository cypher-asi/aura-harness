//! Invariant §4 — full policy enforcement matrix.
//!
//! End-to-end matrix that exercises every combination of
//!
//! * `ActionKind::Delegate` in / out of `allowed_action_kinds`
//! * tool in / out of `allowed_tools` (with `allow_unlisted` toggled)
//! * every [`PermissionLevel`] variant, including `AskOnce` with **and**
//!   without a pre-recorded session approval
//! * runtime-capability ledger satisfied / not-satisfied for a tool
//!   with a declared integration requirement
//!
//! through [`Kernel::process_direct`] on a `ToolProposal` transaction
//! (the real production path that backs Invariant §4) and asserts the
//! resulting [`RecordEntry`]'s [`Decision`] carries the expected
//! accept / reject payload. Return values are a secondary check — the
//! authoritative evidence is the record log entry, because that is
//! what Invariant §5 promises downstream auditors.
//!
//! Enforcement target: Invariant §4 + §4.a in `docs/invariants.md`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aura_core::{
    ActionKind, AgentId, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolIntegrationRequirement, RuntimeCapabilityInstall, SystemKind, ToolProposal,
    Transaction, TransactionType,
};
use aura_kernel::{ExecutorRouter, Kernel, KernelConfig, PermissionLevel, PolicyConfig};
use aura_reasoner::{MockProvider, ModelProvider};
use aura_store::{RocksStore, Store};
use tempfile::TempDir;

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

const TARGET_TOOL: &str = "some_tool";

fn make_kernel(policy: PolicyConfig) -> (Kernel, TempDir, TempDir) {
    let db_dir = TempDir::new().expect("tempdir db");
    let ws_dir = TempDir::new().expect("tempdir ws");
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).expect("rocks"));
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).expect("kernel::new");
    (kernel, db_dir, ws_dir)
}

fn proposal_tx(agent_id: AgentId) -> Transaction {
    let proposal = ToolProposal::new(
        "tool-use-matrix",
        TARGET_TOOL,
        serde_json::json!({ "path": "f.txt" }),
    );
    Transaction::tool_proposal(agent_id, &proposal).expect("serialize proposal")
}

fn all_action_kinds() -> HashSet<ActionKind> {
    let mut set = HashSet::new();
    set.insert(ActionKind::Reason);
    set.insert(ActionKind::Memorize);
    set.insert(ActionKind::Decide);
    set.insert(ActionKind::Delegate);
    set
}

fn allowed_without_delegate() -> HashSet<ActionKind> {
    let mut set = HashSet::new();
    set.insert(ActionKind::Reason);
    set.insert(ActionKind::Memorize);
    set.insert(ActionKind::Decide);
    set
}

// ---------------------------------------------------------------------
// Matrix table
// ---------------------------------------------------------------------

/// Expected outcome for a single matrix row.
#[derive(Debug, Clone)]
enum Expected {
    /// Kernel records an accepted decision (one accepted action id,
    /// no rejected proposals).
    Accepted,
    /// Kernel records a rejected decision whose reason contains
    /// `substring`.
    Rejected { substring: &'static str },
}

// The matrix rows. Each row is uniquely identifiable by `name` so a
// failure message tells you exactly which cell of the table regressed.
fn rows() -> Vec<(&'static str, PolicyConfig, Expected, Option<Pre>)> {
    use Expected::{Accepted, Rejected};

    // Helper closures for integration-requirement rows.
    let brave_requirement = InstalledToolIntegrationRequirement {
        integration_id: None,
        provider: Some("brave_search".to_string()),
        kind: Some("workspace_integration".to_string()),
    };
    let brave_installed = InstalledIntegrationDefinition {
        integration_id: "integration-brave-1".to_string(),
        name: "Brave Search".to_string(),
        provider: "brave_search".to_string(),
        kind: "workspace_integration".to_string(),
        metadata: HashMap::new(),
    };

    let mut out: Vec<(&'static str, PolicyConfig, Expected, Option<Pre>)> = Vec::new();

    let policy_with_level = |level: PermissionLevel| -> PolicyConfig {
        let mut cfg = PolicyConfig::default().with_tool_permission(TARGET_TOOL, level);
        cfg.allowed_tools.insert(TARGET_TOOL.into());
        cfg
    };

    // ---- Action-kind axis ------------------------------------------
    out.push((
        "delegate_action_kind_disallowed/tool_listed/always_allow",
        {
            let mut cfg = PolicyConfig {
                allowed_action_kinds: allowed_without_delegate(),
                ..PolicyConfig::default()
            };
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg
        },
        Rejected {
            substring: "Action kind",
        },
        None,
    ));

    out.push((
        "delegate_action_kind_allowed/tool_unlisted/allow_unlisted_true",
        PolicyConfig {
            allowed_tools: HashSet::new(),
            allowed_action_kinds: all_action_kinds(),
            allow_unlisted: true,
            ..PolicyConfig::default()
        },
        Accepted,
        None,
    ));

    out.push((
        "delegate_action_kind_allowed/tool_unlisted/allow_unlisted_false",
        {
            let mut cfg = PolicyConfig::restrictive();
            cfg.allowed_action_kinds = all_action_kinds();
            cfg
        },
        Rejected {
            substring: "not allowed",
        },
        None,
    ));

    // ---- Permission-level axis (tool listed) ------------------------

    out.push((
        "permission_always_allow/tool_listed",
        policy_with_level(PermissionLevel::AlwaysAllow),
        Accepted,
        None,
    ));

    out.push((
        "permission_deny/tool_listed",
        policy_with_level(PermissionLevel::Deny),
        Rejected {
            substring: "not allowed",
        },
        None,
    ));

    out.push((
        "permission_always_ask/tool_listed",
        policy_with_level(PermissionLevel::AlwaysAsk),
        Rejected {
            substring: "requires approval for each use",
        },
        None,
    ));

    out.push((
        "permission_ask_once_without_approval/tool_listed",
        policy_with_level(PermissionLevel::AskOnce),
        Rejected {
            substring: "requires approval",
        },
        None,
    ));

    out.push((
        "permission_ask_once_with_approval/tool_listed",
        policy_with_level(PermissionLevel::AskOnce),
        Accepted,
        Some(Pre::ApproveForSession(TARGET_TOOL)),
    ));

    // ---- Permission-level axis (tool UNLISTED) ----------------------
    // With allow_unlisted=false every permission level (except the
    // permissive-by-default override) should deny.
    out.push((
        "permission_default_deny/tool_unlisted_restrictive",
        {
            let mut cfg = PolicyConfig::restrictive();
            cfg.allowed_action_kinds = all_action_kinds();
            // No tool added to the allow list.
            cfg
        },
        Rejected {
            substring: "not allowed",
        },
        None,
    ));

    out.push((
        "permission_always_ask_override/tool_unlisted/allow_unlisted_true",
        {
            let mut cfg = PolicyConfig::default()
                .with_tool_permission(TARGET_TOOL, PermissionLevel::AlwaysAsk);
            cfg.allowed_tools = HashSet::new();
            cfg.allow_unlisted = true;
            cfg
        },
        Rejected {
            substring: "requires approval for each use",
        },
        None,
    ));

    // ---- Integration / runtime-capability axis ----------------------
    out.push((
        "integration_required/runtime_ledger_absent",
        {
            let mut cfg = PolicyConfig::default();
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg.set_tool_integration_requirements([(
                TARGET_TOOL.to_string(),
                brave_requirement.clone(),
            )]);
            cfg
        },
        Rejected {
            substring: "installed integration",
        },
        None,
    ));

    out.push((
        "integration_required/config_installed_list_satisfied",
        {
            let mut cfg = PolicyConfig::default();
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg.set_installed_integrations([brave_installed.clone()]);
            cfg.set_tool_integration_requirements([(
                TARGET_TOOL.to_string(),
                brave_requirement.clone(),
            )]);
            cfg
        },
        Accepted,
        None,
    ));

    out.push((
        "integration_required/runtime_ledger_empty_denies",
        {
            let mut cfg = PolicyConfig::default();
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg.set_installed_integrations([brave_installed.clone()]);
            cfg.set_tool_integration_requirements([(
                TARGET_TOOL.to_string(),
                brave_requirement.clone(),
            )]);
            cfg
        },
        Rejected {
            substring: "kernel runtime capability ledger",
        },
        Some(Pre::InstallRuntimeCapabilities(RuntimeCapabilityInstall {
            system_kind: SystemKind::CapabilityInstall,
            scope: "session".to_string(),
            session_id: Some("session-1".to_string()),
            installed_integrations: vec![],
            installed_tools: vec![],
        })),
    ));

    out.push((
        "integration_required/runtime_ledger_covers_tool",
        {
            let mut cfg = PolicyConfig::default();
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg.set_tool_integration_requirements([(
                TARGET_TOOL.to_string(),
                brave_requirement.clone(),
            )]);
            cfg
        },
        Accepted,
        Some(Pre::InstallRuntimeCapabilities(RuntimeCapabilityInstall {
            system_kind: SystemKind::CapabilityInstall,
            scope: "session".to_string(),
            session_id: Some("session-1".to_string()),
            installed_integrations: vec![brave_installed.clone()],
            installed_tools: vec![InstalledToolCapability {
                name: TARGET_TOOL.to_string(),
                required_integration: Some(brave_requirement.clone()),
            }],
        })),
    ));

    // ---- Combined stress rows --------------------------------------
    out.push((
        "action_kind_disallowed_dominates_even_if_tool_ok",
        {
            let mut cfg = PolicyConfig {
                allowed_action_kinds: allowed_without_delegate(),
                ..PolicyConfig::default()
            };
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg
        },
        Rejected {
            substring: "Action kind",
        },
        None,
    ));

    out.push((
        "action_kind_disallowed_dominates_even_if_ask_once_approved",
        {
            let mut cfg = PolicyConfig {
                allowed_action_kinds: allowed_without_delegate(),
                ..PolicyConfig::default()
            }
            .with_tool_permission(TARGET_TOOL, PermissionLevel::AskOnce);
            cfg.allowed_tools.insert(TARGET_TOOL.into());
            cfg
        },
        Rejected {
            substring: "Action kind",
        },
        Some(Pre::ApproveForSession(TARGET_TOOL)),
    ));

    // Default `with_defaults` permissive preset: explicitly pin the
    // production defaults that ship with the kernel.
    out.push((
        "defaults/some_tool_unlisted_allow_unlisted_true",
        PolicyConfig::default(),
        Accepted,
        None,
    ));

    // Explicit tool-added row covering the common "installed tool"
    // path used by the bootstrap.
    out.push((
        "defaults/tool_added_via_add_allowed_tool",
        {
            let mut cfg = PolicyConfig::default();
            cfg.add_allowed_tool(TARGET_TOOL);
            cfg
        },
        Accepted,
        None,
    ));

    out
}

/// Pre-flight hook applied to a fresh kernel before the proposal is
/// submitted.
#[derive(Debug, Clone)]
enum Pre {
    ApproveForSession(&'static str),
    InstallRuntimeCapabilities(RuntimeCapabilityInstall),
}

async fn apply_pre(kernel: &Kernel, pre: &Pre) {
    match pre {
        Pre::ApproveForSession(tool) => {
            kernel.policy().approve_for_session(tool);
        }
        Pre::InstallRuntimeCapabilities(install) => {
            let payload = serde_json::to_vec(install).expect("serialize RuntimeCapabilityInstall");
            let tx =
                Transaction::new_chained(kernel.agent_id, TransactionType::System, payload, None);
            kernel
                .process_direct(tx)
                .await
                .expect("install runtime capabilities");
        }
    }
}

// ---------------------------------------------------------------------
// The actual parameterised test
// ---------------------------------------------------------------------

#[tokio::test]
async fn policy_matrix_covers_all_outcomes() {
    let rows = rows();
    assert!(
        rows.len() >= 16,
        "matrix should cover at least 16 rows, got {}",
        rows.len()
    );

    for (name, policy_cfg, expected, pre) in rows {
        let (kernel, _db, _ws) = make_kernel(policy_cfg);

        if let Some(pre) = &pre {
            apply_pre(&kernel, pre).await;
        }

        let tx = proposal_tx(kernel.agent_id);
        let result = kernel
            .process_direct(tx)
            .await
            .unwrap_or_else(|e| panic!("[{name}] process_direct failed: {e}"));

        // Primary assertion — the persisted record entry's Decision.
        let entries = kernel
            .store()
            .scan_record(kernel.agent_id, 0, 64)
            .expect("scan_record");
        let proposal_entry = entries
            .iter()
            .rev()
            .find(|e| e.tx.tx_type == TransactionType::ToolProposal)
            .unwrap_or_else(|| panic!("[{name}] no ToolProposal record entry found"));

        match &expected {
            Expected::Accepted => {
                assert_eq!(
                    proposal_entry.decision.accepted_action_ids.len(),
                    1,
                    "[{name}] expected exactly one accepted action, got {:?}",
                    proposal_entry.decision
                );
                assert!(
                    proposal_entry.decision.rejected.is_empty(),
                    "[{name}] expected no rejections, got {:?}",
                    proposal_entry.decision.rejected
                );
                assert_eq!(
                    proposal_entry.actions.len(),
                    1,
                    "[{name}] expected one authorized action"
                );
                assert_eq!(
                    proposal_entry.effects.len(),
                    1,
                    "[{name}] expected one executed effect"
                );

                let tool_output = result
                    .tool_output
                    .as_ref()
                    .unwrap_or_else(|| panic!("[{name}] missing tool_output"));
                // For Accepted rows we don't constrain is_error — the
                // underlying tool may still fail (e.g. no such tool
                // installed), but the *policy decision* is accept,
                // which is what Invariant §4 governs.
                let _ = tool_output;
            }
            Expected::Rejected { substring } => {
                assert!(
                    proposal_entry.decision.accepted_action_ids.is_empty(),
                    "[{name}] expected no accepted actions, got {:?}",
                    proposal_entry.decision.accepted_action_ids
                );
                assert_eq!(
                    proposal_entry.decision.rejected.len(),
                    1,
                    "[{name}] expected one rejection, got {:?}",
                    proposal_entry.decision.rejected
                );
                let rejection = &proposal_entry.decision.rejected[0];
                assert!(
                    rejection.reason.contains(substring),
                    "[{name}] rejection reason {:?} did not contain expected substring {:?}",
                    rejection.reason,
                    substring
                );
                assert!(
                    proposal_entry.actions.is_empty(),
                    "[{name}] rejected proposal must not produce actions"
                );
                assert!(
                    proposal_entry.effects.is_empty(),
                    "[{name}] rejected proposal must not produce effects"
                );

                let tool_output = result
                    .tool_output
                    .as_ref()
                    .unwrap_or_else(|| panic!("[{name}] missing tool_output"));
                assert!(
                    tool_output.is_error,
                    "[{name}] tool_output must be marked is_error on rejection"
                );
                assert!(
                    tool_output.content.contains(substring),
                    "[{name}] tool_output.content {:?} did not contain {:?}",
                    tool_output.content,
                    substring
                );
            }
        }
    }
}
