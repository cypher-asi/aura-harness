use super::*;

pub(super) fn activity_lines(
    activity: &Activity,
    expanded: bool,
    number: Option<StepNumber>,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    super::super::activity_lines(activity, expanded.into(), number, spinner, width, theme).0
}

pub(super) fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

pub(super) fn text(line: &Line<'_>) -> String {
    texts(std::slice::from_ref(line)).remove(0)
}

pub(super) fn tool_step(
    name: &str,
    args: &str,
    status: ToolStatus,
    result: Option<&str>,
) -> Activity {
    Activity::Tool(ToolStep {
        id: "t1".to_string(),
        name: name.to_string(),
        args: args.to_string(),
        status,
        result: result.map(str::to_string),
        retry: None,
        started: None,
        duration_ms: Some(300),
        diff: None,
        policy: None,
        awaiting_approval: false,
        live: LiveOutput::default(),
    })
}

pub(super) fn tool(status: ToolStatus, result: Option<&str>) -> Activity {
    tool_step("read_file", r#"{"path":"src/main.rs"}"#, status, result)
}

pub(super) fn thought(ms: u64) -> Activity {
    Activity::Thinking(ThinkingStep {
        text: "hmm".to_string(),
        started: None,
        duration_ms: Some(ms),
        running: false,
    })
}
