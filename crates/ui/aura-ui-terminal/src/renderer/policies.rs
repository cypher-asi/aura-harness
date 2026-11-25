//! The Kernel panel's POLICIES tab: what the kernel of the agent on screen
//! lets it do. A read-only document in sections: STATUS, CAPABILITIES,
//! TOOLS, ALWAYS ALLOW, SESSION GRANTS and INTEGRATIONS.

use super::header::HeaderInfo;
use super::panels::{render_scrollbar, ScrollView};
use super::processes::format_elapsed;
use super::side_list::{render_empty, render_hint, split_hint, POLICIES_HINT};
use crate::app::PanelFocus;
use crate::events::{PolicyView, ToolPolicyState};
use crate::mode_command::ApprovalLevel;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Width of the label column of a STATUS row.
const LABEL_WIDTH: usize = 10;

/// The header's info cell: `AGENT/ASK`, in the accent under `yolo`.
pub(super) fn header_info(app: &App, theme: &Theme) -> Option<HeaderInfo> {
    let view = app.policies()?;
    let color = if view.approval == ApprovalLevel::Yolo {
        theme.colors.success
    } else {
        theme.colors.muted
    };
    Some(HeaderInfo {
        text: format!("{}/{}", view.mode.name(), view.approval.name()).to_ascii_uppercase(),
        short: view.approval.name().to_ascii_uppercase(),
        color,
    })
}

/// Draw the policy into `inner`, the body below the tab bar of the panel
/// bordered by `area`.
pub(super) fn render_policies(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Records;
    let Some(view) = app.policies() else {
        render_empty(frame, inner, "No kernel open for this agent.", theme);
        return;
    };
    let lines = policy_lines(view, usize::from(inner.width), theme);
    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = usize::from(rows_area.height);
    let total = lines.len();
    let first = app.sync_policies_scroll(total, visible);
    let shown: Vec<Line> = lines.into_iter().skip(first).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), rows_area);
    let scroll = ScrollView {
        total,
        visible,
        first,
    };
    render_scrollbar(frame, area, rows_area, scroll, is_focused, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, POLICIES_HINT, theme);
    }
}

/// Every line of the document for a body `width` columns wide.
fn policy_lines(view: &PolicyView, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.colors.muted);
    let text = Style::default().fg(theme.colors.foreground);
    let mut lines = Vec::new();

    lines.push(heading("STATUS", None, theme));
    let approval_style = if view.approval == ApprovalLevel::Yolo {
        Style::default().fg(theme.colors.success)
    } else {
        text
    };
    lines.push(row(
        "mode",
        &format!("{} · {}", view.mode.name(), view.mode.description()),
        text,
        width,
        theme,
    ));
    lines.push(row(
        "approval",
        &format!("{} · {}", view.approval.name(), view.approval.description()),
        approval_style,
        width,
        theme,
    ));
    let (commands, commands_style) = if view.commands_confined {
        ("confined to the folder".to_string(), text)
    } else {
        (
            "can't be confined here".to_string(),
            Style::default().fg(theme.colors.success),
        )
    };
    lines.push(row("commands", &commands, commands_style, width, theme));
    let (prompts, prompts_style) = if view.live_prompts {
        (
            format!(
                "live, denied after {}",
                format_elapsed(view.prompt_timeout_secs.saturating_mul(1000))
            ),
            text,
        )
    } else {
        (
            "none: ask calls are denied".to_string(),
            Style::default().fg(theme.colors.success),
        )
    };
    lines.push(row("prompts", &prompts, prompts_style, width, theme));
    let hooks = match view.permission_hooks {
        0 => "none".to_string(),
        1 => "1 permission hook".to_string(),
        n => format!("{n} permission hooks"),
    };
    lines.push(row("hooks", &hooks, text, width, theme));
    lines.push(row(
        "actions",
        &format!(
            "{} · up to {} per reply",
            or_none(&view.action_kinds),
            view.max_proposals
        ),
        text,
        width,
        theme,
    ));
    let hash: String = view.policy_hash.chars().take(8).collect();
    lines.push(row("hash", &or_dash(hash), muted, width, theme));

    lines.push(Line::default());
    let held = format!("{}/{}", view.capabilities.len(), view.capability_total);
    lines.push(heading("CAPABILITIES", Some(held), theme));
    if view.capabilities.is_empty() {
        lines.push(indented("none", muted, width));
    } else {
        for line in wrap(&view.capabilities.join(", "), width.saturating_sub(2)) {
            lines.push(indented(&line, text, width));
        }
    }
    lines.push(row("scope", &view.scope, text, width, theme));

    lines.push(Line::default());
    let (on, ask, off) = view.tool_counts();
    lines.push(heading(
        "TOOLS",
        Some(format!("{on} on · {ask} ask · {off} off")),
        theme,
    ));
    lines.push(indented("per tool, before arguments", muted, width));
    let name_width = view
        .tools
        .iter()
        .map(|tool| tool.name.width())
        .max()
        .unwrap_or(0)
        .min(width / 2);
    // Every state is drawn in the accent; the glyph tells them apart.
    let accent = Style::default().fg(theme.colors.success);
    for tool in &view.tools {
        let glyph = match tool.state {
            ToolPolicyState::On => "✓",
            ToolPolicyState::Ask => "?",
            ToolPolicyState::Off => "✗",
        };
        let name = pad(&fit(&tool.name, name_width), name_width);
        let state = pad(tool.state.label(), 4);
        // "  ✓ " + name + " " + state + " "
        let used = 4 + name_width + 1 + 4 + 1;
        let rule = fit(&tool.decided_by, width.saturating_sub(used));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(glyph, accent),
            Span::raw(" "),
            Span::styled(name, text),
            Span::raw(" "),
            Span::styled(state, accent),
            Span::raw(" "),
            Span::styled(rule, muted),
        ]));
    }

    lines.push(Line::default());
    let ignored = (view.approval == ApprovalLevel::Strict).then(|| "ignored at strict".to_string());
    lines.push(heading("ALWAYS ALLOW", ignored, theme));
    list_or_none(&mut lines, &view.always_allow, text, width, theme);

    lines.push(Line::default());
    lines.push(heading("SESSION GRANTS", None, theme));
    list_or_none(&mut lines, &view.session_grants, text, width, theme);

    lines.push(Line::default());
    lines.push(heading("INTEGRATIONS", None, theme));
    list_or_none(&mut lines, &view.integrations, text, width, theme);
    lines
}

/// A section's title, with a muted note after it.
pub(super) fn heading(title: &'static str, note: Option<String>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(theme.colors.success)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(note) = note {
        spans.push(Span::styled(
            format!("  {note}"),
            Style::default().fg(theme.colors.muted),
        ));
    }
    Line::from(spans)
}

/// `  label     value`, the value cut to fit.
fn row(
    label: &'static str,
    value: &str,
    style: Style,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let room = width.saturating_sub(2 + LABEL_WIDTH);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            pad(label, LABEL_WIDTH),
            Style::default().fg(theme.colors.muted),
        ),
        Span::styled(fit(value, room), style),
    ])
}

fn indented(text: &str, style: Style, width: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(fit(text, width.saturating_sub(2)), style),
    ])
}

fn list_or_none(
    lines: &mut Vec<Line<'static>>,
    items: &[String],
    style: Style,
    width: usize,
    theme: &Theme,
) {
    if items.is_empty() {
        let muted = Style::default().fg(theme.colors.muted);
        lines.push(indented("none", muted, width));
    }
    for item in items {
        lines.push(indented(item, style, width));
    }
}

fn or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn or_dash(text: String) -> String {
    if text.is_empty() {
        "—".to_string()
    } else {
        text
    }
}

/// `text` padded with spaces to `width` columns.
fn pad(text: &str, width: usize) -> String {
    let gap = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(gap))
}

/// `text` cut to `width` columns, ending in `…` when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `text` broken at spaces into lines of at most `width` columns; a word
/// longer than that is cut.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let needed = if line.is_empty() {
            word.width()
        } else {
            line.width() + 1 + word.width()
        };
        if needed > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(|line| fit(&line, width)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, ToolPolicyView, UiCommand};
    use crate::mode_command::PermissionMode;
    use crate::{PaneTab, RecordsTab};
    use ratatui::{backend::TestBackend, Terminal};

    fn render_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn tool(name: &str, state: ToolPolicyState, rule: &str) -> ToolPolicyView {
        ToolPolicyView {
            name: name.into(),
            state,
            decided_by: rule.into(),
        }
    }

    fn view(approval: ApprovalLevel) -> PolicyView {
        PolicyView {
            mode: PermissionMode::Agent,
            approval,
            policy_hash: "0123456789abcdef".into(),
            action_kinds: vec!["delegate".into(), "reason".into()],
            max_proposals: 8,
            capabilities: vec!["SpawnAgent".into(), "InvokeProcess".into()],
            capability_total: 12,
            scope: "all orgs, projects and agents".into(),
            tools: vec![
                tool(
                    "read_file",
                    ToolPolicyState::On,
                    "user_default.per_tool[read_file]=on",
                ),
                tool(
                    "write_file",
                    ToolPolicyState::Ask,
                    "user_default.fallback=ask",
                ),
                tool("git_push", ToolPolicyState::Off, "mode.plan"),
            ],
            always_allow: vec!["`cargo test` commands".into()],
            commands_confined: true,
            live_prompts: true,
            prompt_timeout_secs: 600,
            ..PolicyView::default()
        }
    }

    fn app_on_policies() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: "aa".into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        app.select_pane_tab(PaneTab::Records(RecordsTab::Policies));
        app
    }

    #[test]
    fn the_tab_lists_status_capabilities_and_how_each_tool_resolves() {
        let mut app = app_on_policies();
        app.process_command(UiCommand::SetPolicies(view(ApprovalLevel::Ask)));
        let rows = render_rows(&mut app, 240, 50);
        let screen = rows.join("\n");
        assert!(screen.contains("POLICIES"), "{screen}");
        assert!(screen.contains("AGENT/ASK"), "{screen}");
        for section in [
            "STATUS",
            "CAPABILITIES  2/12",
            "TOOLS  1 on · 1 ask · 1 off",
        ] {
            assert!(screen.contains(section), "{section}\n{screen}");
        }
        assert!(screen.contains("0123456"), "{screen}");
        assert!(!screen.contains("0123456789"), "the hash is shortened");
        let write = rows.iter().find(|row| row.contains("write_file")).unwrap();
        assert!(write.contains('?') && write.contains("ask"), "{write}");
        assert!(write.contains("user_default.fallback=ask"), "{write}");
        let push = rows.iter().find(|row| row.contains("git_push")).unwrap();
        assert!(push.contains('✗') && push.contains("mode.plan"), "{push}");
        assert!(
            screen.contains("⇧←→ tabs"),
            "the focused tab shows its keys"
        );

        // The panel is short: the last sections are further down.
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::End,
        ));
        let screen = render_rows(&mut app, 240, 50).join("\n");
        assert!(screen.contains("`cargo test` commands"), "{screen}");
        assert!(screen.contains("INTEGRATIONS"), "{screen}");
    }

    #[test]
    fn strict_flags_the_always_allow_rules_as_ignored() {
        let mut app = app_on_policies();
        app.process_command(UiCommand::SetPolicies(view(ApprovalLevel::Strict)));
        let screen = render_rows(&mut app, 240, 50).join("\n");
        assert!(screen.contains("STRICT │"), "the header names the level");
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::End,
        ));
        let screen = render_rows(&mut app, 240, 50).join("\n");
        assert!(screen.contains("ignored at strict"), "{screen}");
    }

    #[test]
    fn an_agent_without_a_kernel_says_so() {
        let mut app = app_on_policies();
        let screen = render_rows(&mut app, 200, 30).join("\n");
        assert!(
            screen.contains("No kernel open for this agent."),
            "{screen}"
        );
    }

    #[test]
    fn long_text_is_cut_and_wrapped_to_the_panel() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 4), "abc");
        assert_eq!(
            wrap("SpawnAgent, ControlAgent, ReadAgent", 14),
            ["SpawnAgent,", "ControlAgent,", "ReadAgent"]
        );
    }
}
