//! Every tool the catalog can advertise must have an explicit mode-gate
//! classification. Unclassified tools fall back to `Network` (denied in
//! every non-Agent mode), which is safe but silently hides new built-ins
//! from Plan / Ask / Debug sessions; this test forces a deliberate choice.

use aura_agent_kernel::tool_mode_class;
use aura_core_types::AgentPermissions;
use aura_tools::catalog::ToolProfile;
use aura_tools::{ToolCatalog, GIT_TOOL_NAMES};
use std::collections::BTreeSet;

#[test]
fn every_catalog_tool_has_a_mode_classification() {
    let catalog = ToolCatalog::new();
    let full = AgentPermissions::full_access();
    let names: BTreeSet<String> = [ToolProfile::Core, ToolProfile::Agent, ToolProfile::Engine]
        .into_iter()
        .flat_map(|profile| catalog.tools_for_profile_with_permissions(profile, Some(&full)))
        .map(|definition| definition.name)
        .chain(GIT_TOOL_NAMES.iter().map(|name| (*name).to_string()))
        .collect();

    assert!(!names.is_empty(), "catalog should not be empty");
    let unclassified: Vec<&String> = names
        .iter()
        .filter(|name| tool_mode_class(name).is_none())
        .collect();
    assert!(
        unclassified.is_empty(),
        "tools missing a mode-gate classification in \
         aura-agent-kernel/src/policy/check/mode_gate.rs: {unclassified:?}"
    );
}
