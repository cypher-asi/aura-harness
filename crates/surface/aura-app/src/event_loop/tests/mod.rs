use super::*;
use crate::agent_definitions::AgentDefinitions;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use crate::record_loader::json_payload;
use crate::session_helpers::{build_executor_router_with_config, default_agent_config_with_auth};
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_agent_kernel::KernelConfig;
use aura_auth::{Vault, ZosIdentity};
use aura_context_skills::{SkillLoader, SkillManager};
use aura_core_types::{
    Assessment, Difficulty, Exploration, Hash, OutcomePayload, OutcomeSignal, PromptCategory,
    PromptPayload, Route, RouteLevel, Scope, Spec,
};
use aura_exec_tools::ToolConfig;
use aura_gate::live::GateLive;
use aura_model::{
    MockProvider, MockResponse, ModelProvider, ModelRequest, ModelResponse, ReasonerError,
};
use aura_store_db::RocksStore;
use aura_ui::events::{PolicyView, RewindScope, ToolPolicyState};
use aura_ui::{ApprovalLevel, PermissionMode};
use tempfile::TempDir;
use tokio::sync::{Barrier, Semaphore};

mod agent_lists;
mod agent_selection;
mod approval_modes;
mod channel_chat;
mod channel_plan;
mod channel_queue_open;
mod channels_projects;
mod channels_projects_zode;
mod cloud_members;
mod cloud_waiting;
mod commands_and_cancel;
mod compare_runs;
mod delegation;
mod escalation;
mod factory_restart;
mod factory_restart_together;
mod fixture;
mod flow;
mod gate_learning;
mod gate_records;
mod gate_review;
mod gate_routing;
mod helpers;
mod inbox;
mod inbox_mid_turn;
mod inbox_restart;
mod inbox_restart_resume;
mod messages;
mod offline_cloud;
mod open_plan;
mod provider;
mod queue_edits;
mod restart;
mod rewinding;
mod stall_fixes;
mod stalls;
mod subtasks;
pub(in crate::event_loop) mod support;
mod typed_tasks;
mod withdrawn_work;

use fixture::{
    borrowed, completed, drain, fixture, fixture_with_gate, lock_registry_agent, loop_state,
    message, message_with_image, next_done, settle, steady_gate, steady_gate_without_delegation,
    user_images, Fixture,
};
use helpers::{
    checkpointed_turn, gate_on_zos, learn_questions, open_again, outcomes_of, prompts_of,
    record_of, route_lines, system_texts, todo, tool_results, turn, unrestricted, user_texts, yolo,
};
use provider::{mock_assessment, GatedProvider, Sent, SentLog, REFUSING_MODEL};
