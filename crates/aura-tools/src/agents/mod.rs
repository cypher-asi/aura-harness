//! Phase 5 cross-agent tools.
//!
//! Only `spawn_agent` is implemented in this commit. The other four
//! (`send_to_agent`, `agent_lifecycle`, `get_agent_state`, `delegate_task`)
//! are intentionally deferred — see `TODO(phase5-part-2)` markers below.
//!
//! All tools in this module are gated behind the `agent_permissions` Cargo
//! feature on `aura-tools`, which in turn forwards to the matching feature
//! on `aura-kernel`. With the feature off (the phase-5 rollout default) the
//! module is still compiled so its types remain usable, but the tools are
//! **not** registered in the default `ToolCatalog`.

pub mod spawn_agent;

pub use spawn_agent::{SpawnAgentInput, SpawnAgentOutcome, SpawnAgentTool};

// TODO(phase5-part-2): `send_to_agent` — ControlAgent-gated message delivery.
// TODO(phase5-part-2): `agent_lifecycle` — hibernate/stop/restart/wake/start.
// TODO(phase5-part-2): `get_agent_state` — ReadAgent-gated state inspection.
// TODO(phase5-part-2): `delegate_task` — ControlAgent + ActionKind::Delegate.
