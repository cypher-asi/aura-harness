//! `<plan_mode>` section, appended to a turn's system prompt while the
//! session runs in plan mode.
//!
//! The kernel's mode gate already denies code edits in plan mode; this
//! tells the model why, and what it should produce instead: a plan
//! written with `create_plan` for the user to review and build.

/// Render the plan-mode section wrapped in its envelope.
#[must_use]
pub fn render() -> String {
    let body = "\
The session is in plan mode. The user wants a plan before any change is made.
- You cannot edit code, run mutating commands or start agents; such tool calls are denied. Do not try to work around this.
- Investigate first: read files, search the code and run read-only commands until you understand what has to change.
- If the request is ambiguous or has several reasonable implementations that change the plan, ask the user one or two short questions and stop, instead of guessing.
- When you are ready, call `create_plan` with a short name, a one or two sentence overview, the plan in Markdown, and ordered todos. Cite the files to change and the key code; keep it proportional to the task.
- To revise a plan after feedback, call `create_plan` again with `path` set to the existing plan.
- After writing the plan, summarize it in a few sentences and tell the user to run /build when they are happy with it. Do not start implementing.";
    format!("<plan_mode>\n{body}\n</plan_mode>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_returns_envelope_naming_the_plan_tool() {
        let out = render();
        assert!(out.starts_with("<plan_mode>\n"));
        assert!(out.ends_with("\n</plan_mode>"));
        assert!(out.contains("create_plan"));
        assert!(out.contains("/build"));
    }
}
