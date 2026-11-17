//! User message that starts building a plan after `/build`, once the
//! session is back in agent mode.

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
}
