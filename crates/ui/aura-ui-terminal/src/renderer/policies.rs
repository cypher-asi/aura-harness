//! The Kernel panel's POLICIES tab: what the kernel of the agent on screen
//! lets it do. An info document in sections: STATUS, CAPABILITIES, TOOLS,
//! ALWAYS ALLOW, SESSION GRANTS and INTEGRATIONS.

use super::chrome::Body;
use super::header::HeaderInfo;
use super::processes::format_elapsed;
use super::side_list::render_empty;
use crate::app::PanelFocus;
use crate::components::{Info, Value};
use crate::events::{PolicyView, ToolPolicyState};
use crate::mode_command::ApprovalLevel;
use crate::{App, Theme};
use ratatui::{layout::Rect, text::Line, Frame};
use unicode_width::UnicodeWidthStr;

/// The header's info cell: `AGENT/ASK`, muted like every other count.
pub(super) fn header_info(app: &App, theme: &Theme) -> Option<HeaderInfo> {
    let view = app.policies()?;
    Some(HeaderInfo {
        text: format!("{}/{}", view.mode.name(), view.approval.name()).to_ascii_uppercase(),
        short: view.approval.name().to_ascii_uppercase(),
        color: theme.colors.muted,
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
    let Some(view) = app.policies() else {
        render_empty(frame, inner, "No kernel open for this agent.", theme);
        return;
    };
    let body = Body::new(area, inner, app.focus() == PanelFocus::Records);
    let lines = policy_lines(view, body.width(), theme);
    let first = app.sync_policies_scroll(lines.len(), body.visible());
    body.draw_document(frame, lines, first, theme);
}

/// Every line of the document for a body `width` columns wide.
fn policy_lines(view: &PolicyView, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme);

    info.section("STATUS", None);
    info.fact(
        "Mode",
        &format!("{} · {}", view.mode.name(), view.mode.description()),
        Value::Plain,
    );
    info.fact(
        "Approval",
        &format!("{} · {}", view.approval.name(), view.approval.description()),
        Value::flag_if(view.approval == ApprovalLevel::Yolo),
    );
    let commands = if view.commands_confined {
        "confined to the folder"
    } else {
        "can't be confined here"
    };
    info.fact(
        "Commands",
        commands,
        Value::flag_if(!view.commands_confined),
    );
    let prompts = if view.live_prompts {
        format!(
            "live, denied after {}",
            format_elapsed(view.prompt_timeout_secs.saturating_mul(1000))
        )
    } else {
        "none: ask calls are denied".to_string()
    };
    info.fact("Prompts", &prompts, Value::flag_if(!view.live_prompts));
    let (hooks, hooks_kind) = match view.permission_hooks {
        0 => ("none".to_string(), Value::Quiet),
        1 => ("1 permission hook".to_string(), Value::Plain),
        n => (format!("{n} permission hooks"), Value::Plain),
    };
    info.fact("Hooks", &hooks, hooks_kind);
    let kinds = if view.action_kinds.is_empty() {
        "none".to_string()
    } else {
        view.action_kinds.join(", ")
    };
    info.fact(
        "Actions",
        &format!("{kinds} · up to {} per reply", view.max_proposals),
        Value::Plain,
    );
    let hash: String = view.policy_hash.chars().take(8).collect();
    info.fact("Hash", &hash, Value::Quiet);

    let held = format!("{}/{}", view.capabilities.len(), view.capability_total);
    info.section("CAPABILITIES", Some(&held));
    if view.capabilities.is_empty() {
        info.none();
    } else {
        info.text(&view.capabilities.join(", "), Value::Plain);
    }
    info.fact("Scope", &view.scope, Value::Plain);

    let (on, ask, off) = view.tool_counts();
    info.section("TOOLS", Some(&format!("{on} on · {ask} ask · {off} off")));
    info.text("per tool, before arguments", Value::Quiet);
    let name_width = view
        .tools
        .iter()
        .map(|tool| tool.name.width())
        .max()
        .unwrap_or(0)
        .min(width / 2);
    for tool in &view.tools {
        // Every state's glyph is the accent; the glyph tells them apart.
        let glyph = match tool.state {
            ToolPolicyState::On => "✓",
            ToolPolicyState::Ask => "?",
            ToolPolicyState::Off => "✗",
        };
        let detail = format!("{:<3}  {}", tool.state.label(), tool.decided_by);
        info.marked(
            (glyph, theme.colors.success),
            (&tool.name, name_width),
            &detail,
        );
    }

    let ignored = (view.approval == ApprovalLevel::Strict).then_some("ignored at strict");
    info.section("ALWAYS ALLOW", ignored);
    info.items(&view.always_allow);
    info.section("SESSION GRANTS", None);
    info.items(&view.session_grants);
    info.section("INTEGRATIONS", None);
    info.items(&view.integrations);
    info.into_lines()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, ToolPolicyView, UiCommand};
    use crate::mode_command::PermissionMode;
    use crate::{PaneTab, RecordsTab};
    use ratatui::style::Modifier;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    fn render(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows_of(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn render_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        rows_of(&render(app, width, height))
    }

    /// The style of the first cell of `needle` on screen.
    fn style_of(buffer: &Buffer, needle: &str) -> ratatui::style::Style {
        let rows = rows_of(buffer);
        let (y, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")));
        let before = &row[..row.find(needle).unwrap()];
        let x = UnicodeWidthStr::width(before);
        buffer[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())].style()
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
    fn sections_and_the_info_cell_are_gray_and_only_flags_take_the_accent() {
        let theme = Theme::cyber();
        let mut app = app_on_policies();
        app.process_command(UiCommand::SetPolicies(view(ApprovalLevel::Yolo)));
        let buffer = render(&mut app, 240, 50);
        let status = style_of(&buffer, "STATUS");
        assert_eq!(status.fg, Some(theme.colors.secondary));
        assert!(!status.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            style_of(&buffer, "YOLO │").fg,
            Some(theme.colors.muted),
            "yolo shows in its fact, not the header"
        );
        assert_eq!(style_of(&buffer, "yolo · ").fg, Some(theme.colors.success));
        assert_eq!(
            style_of(&buffer, "Approval").fg,
            Some(theme.colors.muted),
            "labels are muted"
        );
        assert_eq!(
            style_of(&buffer, "ask  user_default").fg,
            Some(theme.colors.muted),
            "a tool's state is not colored, only its glyph"
        );
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
}
