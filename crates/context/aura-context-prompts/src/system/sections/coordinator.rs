//! `<coordinator>` section, appended to the system prompt of the user's
//! own agents in agent mode.
//!
//! The main agent coordinates: it plans the work as todos and farms each
//! one out to the permanent agent best suited to it with the `task` tool.
//! The harness marks a todo given to an agent as in progress and, when
//! the agent reports back, as done (or back to pending when it failed),
//! and keeps a task for every request and every spawn. Delegating is
//! recommended, not enforced: the agent keeps its own tools for quick
//! edits and answers simple questions itself. Under Gate the
//! `<gate_lead>` section takes this one's place.

/// Render the coordinator section wrapped in its envelope.
#[must_use]
pub fn render() -> String {
    let body = "\
You coordinate the work and farm it out to agents.
- For work with several steps, first write the plan with `todo_write`: one todo per self-contained piece of work.
- Give each todo to an agent with the `task` tool and pass its id as `todo_id`. Pick `subagent_type` by matching the job against the agents listed on the `task` tool: the one whose description fits the job best, such as an explorer for heavy reading, implement for code changes, tests for tests, a reviewer for a second look and a specialist engineer for work in its stack.
- Give every task its goal, the files involved, constraints, a `check` command that proves it is done (such as a focused test or build), and what to report back.
- Send tasks that do not depend on each other in the same response, or with `spawn_mode` `detached`, so they run side by side.
- When an agent reports back, its todo is updated for you: done when it finished and its check passed, pending again when it did not. Read the report, then revise the remaining todos and send the next tasks.
- Do small edits and answer simple questions yourself; delegating them costs more than it saves.";
    format!("<coordinator>\n{body}\n</coordinator>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_returns_envelope_naming_the_task_and_todo_tools() {
        let out = render();
        assert!(out.starts_with("<coordinator>\n"));
        assert!(out.ends_with("\n</coordinator>"));
        assert!(out.contains("`task`"));
        assert!(out.contains("`todo_write`"));
        assert!(out.contains("`todo_id`"));
        assert!(out.contains("`subagent_type`"));
    }
}
