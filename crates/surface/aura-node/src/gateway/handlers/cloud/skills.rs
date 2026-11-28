//! `PUT /v1/cloud/skills/:agent_id`: the hub's copy of an agent's skill
//! folders, so the node lists them and serves `load_skill` itself
//! ([`aura_context_skills::bundle::apply`]). Answers the bundle's digest.

use super::*;
use aura_core_protocol::cloud::{SkillBundle, SkillsSynced};

/// Where a node keeps the skills its hub sends, in its data directory.
pub const HUB_SKILLS_DIR: &str = "hub-skills";

/// `PUT /v1/cloud/skills/:agent_id`.
pub(in crate::gateway) async fn skills_put_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
    Json(bundle): Json<SkillBundle>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    // A node without skills (an older one is the same to the hub) can't
    // serve them: the hub sends the turn its skills' text instead.
    let Some(manager) = state.skill_manager.clone() else {
        return Err(not_cloud());
    };
    let agent = parse_agent(&agent_hex)?;
    for folder in &bundle.skills {
        let names_ok = aura_context_skills::bundle::inside(&folder.name)
            .is_some_and(|name| name.components().count() == 1);
        let paths_ok = (folder.files.iter())
            .all(|file| aura_context_skills::bundle::inside(&file.path).is_some());
        if !names_ok || !paths_ok {
            return Err(bad(format!(
                "skill `{}` has a path outside its folder",
                folder.name
            )));
        }
    }
    let root = state.config.data_dir.join(HUB_SKILLS_DIR);
    let digest = bundle.digest.clone();
    let applied = tokio::task::spawn_blocking(move || {
        let mut manager = manager.write().unwrap_or_else(PoisonError::into_inner);
        aura_context_skills::bundle::apply(&mut manager, &root, agent, &bundle)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not take the skills: {e}"),
        )
    })?;
    applied.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not take the skills: {e}"),
        )
    })?;
    tracing::info!(%agent, %digest, "Took the hub's skills");
    Ok(Json(SkillsSynced { digest }))
}
