//! User messages about building a plan: the one that starts it after
//! `/build`, once the session is back in agent mode, and the note added
//! to a user's message that carries on with a plan still under way.

/// The prompt that asks the agent to carry out the plan at `plan_path`.
#[must_use]
pub fn build_plan_prompt(plan_path: &str) -> String {
    format!(
        "Implement the plan in `{plan_path}`. Read it first. Your todo list already holds its \
         todos; work through them in order. Before starting a todo, mark it `in_progress` with \
         `todo_write`, and mark it `completed` as soon as it is done (or `cancelled`, with a \
         reason, if it turns out to be unnecessary). The plan file's checklist follows your todo \
         list, so do not edit the plan file yourself. When every todo is closed, summarize what \
         changed."
    )
}

/// The note appended to a user's message while the agent has the plan at
/// `plan_path` open. Without it, a request such as "update todos and keep
/// going" reads as new work, and the model starts a todo list of its own
/// that the plan never sees.
#[must_use]
pub fn continue_plan_note(plan_path: &str) -> String {
    format!(
        "(You are working on the plan in `{plan_path}`. Your todo list holds the plan's todos, \
         and the plan's checklist follows that list. Keep updating those todos with \
         `todo_write`, adding, completing or cancelling them there, rather than starting a \
         separate list. Do not edit the plan file yourself.)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_plan_and_the_todo_tool() {
        let prompt = build_plan_prompt(".aura/plans/x.plan.md");
        assert!(prompt.contains("`.aura/plans/x.plan.md`"));
        assert!(prompt.contains("todo_write"));
        assert!(prompt.contains("in_progress"));
    }

    #[test]
    fn continue_note_names_the_plan_and_keeps_to_its_todos() {
        let note = continue_plan_note(".aura/plans/x.plan.md");
        assert!(note.contains("`.aura/plans/x.plan.md`"));
        assert!(note.contains("todo_write"));
        assert!(note.contains("rather than starting a separate list"));
        assert!(note.contains("Do not edit the plan file"));
        assert!(!note.contains('\n'), "one paragraph after the user's text");
    }
}
