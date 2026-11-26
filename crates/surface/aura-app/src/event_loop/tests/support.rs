//! Helpers shared by the event loop's test modules: those under
//! `event_loop::tests` and the ones beside the code they test, such as
//! `channels/tests.rs`. Each test module builds its own fixture, so these
//! take the [`AgentHub`] the fixtures share rather than a fixture.

use crate::event_loop::subagents::{lock, AgentHub};
use std::path::{Path, PathBuf};

/// A new project in `dir`; returns its id and (canonical) folder.
pub(in crate::event_loop) fn open_project(hub: &AgentHub, dir: &Path) -> (String, PathBuf) {
    let mut registry = lock(&hub.registry);
    let (agent, _) = registry.open_project(dir).unwrap();
    let project_id = agent.project_id.unwrap();
    let folder = registry.project(&project_id).unwrap().folder.clone();
    (project_id, folder)
}
