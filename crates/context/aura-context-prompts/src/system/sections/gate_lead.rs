//! `<gate_lead>` section, appended to the system prompt of the main
//! agent's turns when Gate routes them.
//!
//! Under Gate the main agent is a lead: Gate sends each subtask it
//! delegates to the cheapest model proven on that kind of work, so
//! delegating is cheaper than doing the work on the lead's model. While
//! delegation is enforced the app also takes the lead's file tools away;
//! this tells the model how to work that way.

/// Render the lead section. `enforced` says the lead has no file tools
/// and a limited `run_command`; otherwise delegating is only
/// recommended.
#[must_use]
pub fn render(enforced: bool) -> String {
    let stance = if enforced {
        "You lead this work under Gate: you do not edit files yourself, your reads and searches \
         are capped, and your own commands are limited to checks and inspection. Subagents do the \
         work."
    } else {
        "You lead this work under Gate. Delegating is recommended: Gate runs each subtask on the \
         cheapest model that has proven itself on that kind of work, so it usually costs less \
         than doing the work yourself."
    };
    let body = format!(
        "{stance}
- Split the work into self-contained tasks and send each with the `task` tool. Give every task its goal, the files involved, constraints, a `check` command that proves it is done (such as a focused test or build), and what to report back.
- Send tasks that do not depend on each other in the same response; reading and review tasks run side by side, edits run one at a time.
- Delegate even small edits: Gate sends them to a cheap model. Use an explorer task for heavy reading, implement for changes, tests for tests and reviewer for a second look.
- One coherent change is one task: a rename or the same edit across several files goes to a single implement task, not one per file.
- A task whose check fails is retried once on a stronger model with the files restored; read its result before deciding what to do next.
- Before your final reply, confirm the combined result yourself: run the check that proves the whole request is done and look at what changed. Then say what was changed, file by file, and what you verified; do not pass on a subtask's claims you did not confirm, and name anything left undone or unexpected, such as stray files.
- When the request needs no work, such as a question you can answer from what you know or a quick look, answer it directly."
    );
    format!("<gate_lead>\n{body}\n</gate_lead>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_returns_envelope_naming_the_task_tool_and_check() {
        for enforced in [true, false] {
            let out = render(enforced);
            assert!(out.starts_with("<gate_lead>\n"));
            assert!(out.ends_with("\n</gate_lead>"));
            assert!(out.contains("`task`"));
            assert!(out.contains("`check`"));
            assert!(out.contains("answer it directly"));
            assert!(out.contains("Before your final reply, confirm the combined result"));
            assert!(out.contains("One coherent change is one task"));
        }
        assert!(render(true).contains("you do not edit files yourself"));
        assert!(render(false).contains("Delegating is recommended"));
    }
}
