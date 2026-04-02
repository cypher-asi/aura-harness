//! Skills CRUD API endpoints — list, get, and activate skills.

use super::RouterState;
use aura_skills::{SkillActivation, SkillFrontmatter, SkillMeta, SkillSource};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<serde_json::Value>)>;

fn skill_err(e: aura_skills::SkillError) -> (StatusCode, Json<serde_json::Value>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(serde_json::json!({ "error": msg })))
}

fn require_skills(
    state: &RouterState,
) -> Result<&std::sync::Arc<aura_skills::SkillManager>, (StatusCode, Json<serde_json::Value>)> {
    state.skill_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "skill system not configured" })),
        )
    })
}

/// Response for a single skill (frontmatter + body).
#[derive(Serialize)]
pub(super) struct SkillDetail {
    name: String,
    description: String,
    source: SkillSource,
    body: String,
    frontmatter: SkillFrontmatter,
}

/// Response for skill activation.
#[derive(Serialize)]
pub(super) struct ActivationResponse {
    skill_name: String,
    rendered_content: String,
    allowed_tools: Vec<String>,
    fork_context: bool,
    agent_type: Option<String>,
}

impl From<SkillActivation> for ActivationResponse {
    fn from(a: SkillActivation) -> Self {
        Self {
            skill_name: a.skill_name,
            rendered_content: a.rendered_content,
            allowed_tools: a.allowed_tools,
            fork_context: a.fork_context,
            agent_type: a.agent_type,
        }
    }
}

/// `GET /api/skills` — list all skills (metadata only).
pub(super) async fn list_skills(
    State(state): State<RouterState>,
) -> ApiResult<Vec<SkillMeta>> {
    let mgr = require_skills(&state)?;
    Ok(Json(mgr.list_all()))
}

/// `GET /api/skills/:name` — get full skill details.
pub(super) async fn get_skill(
    State(state): State<RouterState>,
    Path(name): Path<String>,
) -> ApiResult<SkillDetail> {
    let mgr = require_skills(&state)?;
    let skill = mgr.get(&name).map_err(skill_err)?;
    Ok(Json(SkillDetail {
        name: skill.frontmatter.name.clone(),
        description: skill.frontmatter.description.clone(),
        source: skill.source.clone(),
        body: skill.body.clone(),
        frontmatter: skill.frontmatter.clone(),
    }))
}

/// Request body for skill activation.
#[derive(Deserialize)]
pub(super) struct ActivateBody {
    #[serde(default)]
    pub arguments: String,
}

/// `POST /api/skills/:name/activate` — activate a skill with arguments.
pub(super) async fn activate_skill(
    State(state): State<RouterState>,
    Path(name): Path<String>,
    Json(body): Json<ActivateBody>,
) -> ApiResult<ActivationResponse> {
    let mgr = require_skills(&state)?;
    let activation = mgr.activate(&name, &body.arguments).map_err(skill_err)?;
    Ok(Json(activation.into()))
}
