//! A turn's request on the agent's task board (see [`super::super::tasks`]).

use aura_agent::{AgentError, AgentLoopResult};

use super::super::agents::AgentSession;
use super::super::plan_mode::open_plan;
use super::super::tasks::{request_state, summary_of};
use super::{Prompt, Request};

/// Put the turn answering `prompt` on the task board: a new request, or,
/// for a harness notice, the agent's latest task again. `planning` is
/// whether the agent is in plan mode; [`request_plan`] says which plan
/// the request works on.
pub(super) async fn begin_task(session: &AgentSession, prompt: &Prompt, planning: bool) {
    let agent_id = session.agent_id();
    let workspace = session.kernel.agent_workspace();
    match prompt.request {
        Request::Resume => session.tasks.resume_request(agent_id, &workspace).await,
        Request::New | Request::Build => {
            let (plan, builds) = request_plan(session, planning);
            session.tasks.begin_request(
                agent_id,
                workspace,
                &session.identity.name,
                &prompt.text,
                plan,
                builds,
            );
        }
    }
}

/// The plan a new request of `session` works on, and whether the request
/// is that plan's own. In plan mode (`planning`) a request revises the
/// agent's open plan ([`open_plan`]), so it files under that plan as a
/// task of its own. While the agent builds its plan every request is the
/// plan's, as the first one was: follow-ups such as "keep going" carry
/// the same todo list, which the plan's todos already show.
fn request_plan(session: &AgentSession, planning: bool) -> (Option<String>, bool) {
    if planning {
        let workspace = session.kernel.agent_workspace();
        let plan = session
            .active_plan
            .clone()
            .filter(|path| open_plan(&workspace, Some(path)).is_some());
        return (plan, false);
    }
    if session.building_plan {
        return (session.active_plan.clone(), true);
    }
    (None, false)
}

/// Close the task board's request of the turn that ended with `result`,
/// which the user stopped when `cancelled`.
pub(super) async fn end_request(
    session: &AgentSession,
    result: &Result<AgentLoopResult, AgentError>,
    cancelled: bool,
) {
    let summary = result
        .as_ref()
        .ok()
        .and_then(|result| summary_of(&result.total_text));
    let failed = result.is_err() && !cancelled;
    session
        .tasks
        .end_request(
            session.agent_id(),
            |items| request_state(cancelled, failed, items),
            summary,
        )
        .await;
}
