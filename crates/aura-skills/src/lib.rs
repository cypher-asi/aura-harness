//! # aura-skills
//!
//! Skill system for Aura agents, fully compatible with the Claude Code
//! `SKILL.md` / `AgentSkills` open standard.
//!
//! A *skill* is an authored, versioned package of instructions plus supporting
//! files that gets installed on an agent. Skills are discovered from multiple
//! filesystem locations with a precedence order:
//!
//! 1. **Workspace** — `{workspace}/skills/`
//! 2. **Agent-personal** — `~/.aura/agents/{id}/skills/`
//! 3. **Personal** — `~/.aura/skills/`
//! 4. **Extra** — arbitrary directories from config
//! 5. **Bundled** — shipped with the runtime
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use aura_skills::{SkillLoader, SkillManager};
//! use aura_skills::loader::SkillLoaderConfig;
//! use std::path::PathBuf;
//!
//! let loader = SkillLoader::with_defaults(
//!     Some(PathBuf::from(".")),
//!     Some("agent-1"),
//! );
//! let manager = SkillManager::new(loader);
//!
//! // Inject skill catalogue into the system prompt
//! let mut prompt = String::from("You are an assistant.");
//! manager.inject_skills(&mut prompt);
//!
//! // Activate a skill
//! let activation = manager.activate("deploy", "production us-east-1");
//! ```

#![allow(clippy::module_name_repetitions)]

pub mod activation;
pub mod error;
pub mod install;
pub mod loader;
pub mod manager;
pub mod parser;
pub mod prompt;
pub mod registry;
pub mod types;

pub use error::SkillError;
pub use install::{SkillInstallStore, SkillInstallation};
pub use loader::SkillLoader;
pub use manager::{AgentSkillPermissions, SkillManager};
pub use registry::SkillRegistry;
pub use types::{Skill, SkillActivation, SkillFrontmatter, SkillMeta, SkillSource};
