//! Recording where a plan came from, through `create_plan`.

use super::*;
use crate::sandbox::Sandbox;
use chrono::{TimeZone, Utc};
use serde_json::json;
use tempfile::TempDir;

fn origin(hour: u32, author: &str, model: &str) -> PlanOrigin {
    PlanOrigin {
        at: Utc.with_ymd_and_hms(2026, 11, 28, hour, 32, 0).unwrap(),
        author: Some(author.to_string()),
        model: Some(model.to_string()),
        machine: "aura-rus199-12b4".to_string(),
    }
}

fn args(path: Option<&str>) -> serde_json::Value {
    let mut args = json!({
        "name": "Plan meta",
        "overview": "Show where plans come from",
        "plan": "## Why\n\nBecause.",
        "todos": [{"id": "a", "content": "first"}]
    });
    if let Some(path) = path {
        args["path"] = json!(path);
    }
    args
}

fn written(dir: &TempDir, output: &[u8]) -> (String, PlanFile) {
    let text = String::from_utf8_lossy(output).to_string();
    let path = plan_path_from_output(&text).unwrap().to_string();
    let plan = PlanFile::load(&dir.path().join(&path)).unwrap();
    (path, plan)
}

#[test]
fn a_new_plan_records_its_origin() {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let result = create_plan_from(&sandbox, &args(None), &origin(14, "ab12", "opus")).unwrap();
    let (_, plan) = written(&dir, &result.stdout);
    let meta = &plan.meta;
    assert_eq!(meta.created_at.as_deref(), Some("2026-11-28T14:32:00Z"));
    assert_eq!(meta.updated_at, None, "never rewritten");
    assert_eq!(meta.author.as_deref(), Some("ab12"));
    assert_eq!(meta.model.as_deref(), Some("opus"));
    assert_eq!(meta.machine.as_deref(), Some("aura-rus199-12b4"));
}

#[test]
fn a_rewrite_keeps_creation_and_records_the_update() {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let first = create_plan_from(&sandbox, &args(None), &origin(14, "ab12", "opus")).unwrap();
    let (path, _) = written(&dir, &first.stdout);
    let mut later = origin(16, "ff00", "sonnet");
    later.machine = "other-box".to_string();
    let again = create_plan_from(&sandbox, &args(Some(&path)), &later).unwrap();
    let (_, plan) = written(&dir, &again.stdout);
    let meta = &plan.meta;
    assert_eq!(meta.created_at.as_deref(), Some("2026-11-28T14:32:00Z"));
    assert_eq!(meta.updated_at.as_deref(), Some("2026-11-28T16:32:00Z"));
    assert_eq!(meta.author.as_deref(), Some("ab12"), "the author stays");
    assert_eq!(meta.machine.as_deref(), Some("aura-rus199-12b4"));
    assert_eq!(meta.model.as_deref(), Some("sonnet"), "the latest model");
}

#[test]
fn rewriting_an_old_plan_fills_in_what_it_lacks() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
    let path = format!("{PLANS_DIR}/old_00000001{PLAN_SUFFIX}");
    let old = "---\nname: Old\ntodos:\n- id: a\n  content: first\n---\n\nBody.\n";
    std::fs::write(dir.path().join(&path), old).unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let result =
        create_plan_from(&sandbox, &args(Some(&path)), &origin(9, "ab12", "opus")).unwrap();
    let (_, plan) = written(&dir, &result.stdout);
    assert_eq!(
        plan.meta.created_at.as_deref(),
        Some("2026-11-28T09:32:00Z")
    );
    assert_eq!(
        plan.meta.updated_at.as_deref(),
        Some("2026-11-28T09:32:00Z")
    );
    assert_eq!(plan.meta.author.as_deref(), Some("ab12"));
}

#[test]
fn the_origin_round_trips_and_sits_above_the_todos() {
    let mut meta = PlanMeta {
        name: "Round".into(),
        ..PlanMeta::default()
    };
    origin(14, "ab12", "opus").stamp(&mut meta, None);
    let plan = PlanFile {
        meta,
        body: "Body.".into(),
    };
    let text = plan.render();
    assert_eq!(PlanFile::parse(&text).unwrap(), plan);
    let created = text.find("created_at:").unwrap();
    let todos = text.find("todos:").unwrap();
    assert!(created < todos, "{text}");
}

#[test]
fn an_old_plan_parses_and_renders_with_no_new_keys() {
    let old = "---\nname: Old\noverview: Before origins\ntodos:\n- id: a\n  content: first\n  status: completed\n---\n\nBody.\n";
    let plan = PlanFile::parse(old).unwrap();
    assert_eq!(plan.meta.created_at, None);
    assert_eq!(plan.meta.author, None);
    assert_eq!(plan.meta.machine, None);
    let text = plan.render();
    for key in [
        "created_at",
        "updated_at",
        "author",
        "model",
        "machine",
        "null",
    ] {
        assert!(!text.contains(key), "{key} in:\n{text}");
    }
}

#[test]
fn syncing_todos_is_not_an_update() {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let result = create_plan_from(&sandbox, &args(None), &origin(14, "ab12", "opus")).unwrap();
    let (path, plan) = written(&dir, &result.stdout);
    let mut items = todo_items(&plan);
    items[0].status = TodoStatus::Completed;
    assert!(sync_todo_list(&dir.path().join(&path), &items).unwrap());
    let (_, synced) = written(&dir, &result.stdout);
    assert_eq!(synced.meta.updated_at, None);
    assert_eq!(synced.meta.created_at, plan.meta.created_at);
}

#[test]
fn the_tool_call_names_its_author_and_model() {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let mut ctx = crate::tool::ToolContext::new(sandbox, crate::ToolConfig::default());
    let agent = aura_core_types::AgentId::new([7; 32]);
    ctx.caller_agent_id = Some(agent);
    ctx.caller_model_id = Some(" opus ".to_string());
    let origin = PlanOrigin::of_call(&ctx);
    assert_eq!(origin.author, Some(agent.to_hex()));
    assert_eq!(origin.model.as_deref(), Some("opus"));
    ctx.caller_model_id = Some("  ".to_string());
    assert_eq!(PlanOrigin::of_call(&ctx).model, None);
}
