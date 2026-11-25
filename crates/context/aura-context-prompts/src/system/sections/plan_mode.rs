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
- Write the plan as a short document a person reads from top to bottom, not a checklist. The tool adds the title, overview and todo checklist, so start the body with `##` sections whose headings say what the section is about (for example \"How it works today\", \"Phase 1: Measure first\", \"What could go wrong\") rather than generic labels. Open with a paragraph of context, then write plain sentences in short paragraphs. Use a bullet list for steps or parallel items, each bullet a full sentence. Use `inline code` only for what the reader would type or search for: file paths, commands, and names of functions, types and settings; write everything else as ordinary text. Bold a key phrase sparingly. Add a fenced code block with a language tag only when the exact code matters, a table only to compare several items across the same columns, and a mermaid diagram when a flow is easier to see than to read. Keep each todo one short line.
- To revise a plan after feedback, call `create_plan` again with `path` set to the existing plan.
- After writing the plan, summarize it in a few sentences and tell the user to say \"implement\" (or run /build) when they are happy with it, or /plan to leave plan mode without building. Do not start implementing.";
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
        assert!(out.contains("/plan"));
    }

    #[test]
    fn render_asks_for_a_readable_document_with_sparing_code() {
        let out = render();
        assert!(out.contains("short document a person reads from top to bottom"));
        assert!(out.contains("`inline code` only for what the reader would type or search for"));
        assert!(
            !out.contains("Approach, Files to change, Risks"),
            "generic section labels invite checklist-style plans"
        );
    }
}
