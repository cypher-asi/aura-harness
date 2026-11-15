//! The conversation: messages, agent steps and shell commands.
//!
//! Only the last [`App::visible`] entries are laid out; "Show earlier"
//! pages further back, so a long conversation costs as much per frame as
//! a short one.

use super::{ansi_text, clip_lines, duration, muted, spinner};
use crate::app::{App, Message, TRANSCRIPT_ID};
use crate::draw;
use crate::model::{Body, Entry, Level, Reply, Shell, Thinking, Tool, ToolStatus};
use crate::theme;
use aura_ui_core::format::{format_tool_label, parse_tool_args};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, canvas, column, container, markdown, row, scrollable, space, text, Column,
};
use iced::{Alignment, Color, Element, Fill};

/// Lines of a tool result or command output shown before the rest is cut.
const MAX_OUTPUT_LINES: usize = 400;
/// Widest the conversation column grows.
const MAX_WIDTH: f32 = 980.0;
/// Row height of one diff line (mono text plus leading).
const DIFF_LINE_HEIGHT: f32 = 18.0;

pub fn view(app: &App) -> Element<'_, Message> {
    let entries = &app.model.entries;
    let start = entries.len().saturating_sub(app.visible);

    let mut items = Column::new()
        .spacing(14)
        .padding([24, 28])
        .max_width(MAX_WIDTH);
    if entries.is_empty() {
        items = items.push(welcome(app));
    }
    if start > 0 {
        items = items.push(
            container(
                button(muted(format!("Show earlier ({start} hidden)")))
                    .style(theme::ghost)
                    .on_press(Message::ShowEarlier),
            )
            .center_x(Fill),
        );
    }
    for entry in entries.range(start..) {
        items = items.push(entry_view(app, entry));
    }

    scrollable(container(items).center_x(Fill))
        .id(TRANSCRIPT_ID)
        .anchor_bottom()
        .height(Fill)
        .into()
}

fn welcome(app: &App) -> Element<'_, Message> {
    let aura = canvas(draw::Aura {
        t: app.clock(),
        busy: false,
    })
    .width(72)
    .height(72);
    let hint = |keys: &'static str, what: &'static str| {
        row![
            text(keys)
                .font(theme::mono_medium())
                .size(12)
                .color(theme::BLUE),
            muted(what)
        ]
        .spacing(10)
    };
    container(
        column![
            aura,
            text("What should we build?")
                .size(24)
                .font(theme::ui_semibold())
                .color(theme::WHITE),
            muted("Ask anything, or start with a command."),
            column![
                hint("/", "commands"),
                hint("!cmd", "run a shell command"),
                hint("Ctrl+K", "pick a model"),
                hint("Ctrl+B", "toggle the side panel"),
            ]
            .spacing(6)
            .padding([12, 0]),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .center_x(Fill)
    .padding([80, 0])
    .into()
}

fn entry_view<'a>(app: &'a App, entry: &'a Entry) -> Element<'a, Message> {
    match &entry.body {
        Body::User(text) => user(entry, text),
        Body::Assistant(reply) => assistant(app, entry, reply),
        Body::System { text, level } => system(text, *level),
        Body::Thinking(thinking) => thinking_view(app, entry, thinking),
        Body::Tool(tool) => tool_view(app, entry, tool),
        Body::Shell(shell) => shell_view(app, shell),
    }
}

fn timestamp<'a>(entry: &Entry) -> text::Text<'a> {
    text(entry.at.format("%H:%M").to_string())
        .size(11)
        .color(theme::FAINT)
}

fn user<'a>(entry: &'a Entry, content: &'a str) -> Element<'a, Message> {
    column![
        row![
            text("You")
                .font(theme::ui_semibold())
                .size(12)
                .color(theme::WHITE),
            space::horizontal(),
            timestamp(entry),
        ],
        container(text(content).size(theme::TEXT_SIZE).color(theme::WHITE))
            .padding([10, 14])
            .width(Fill)
            .style(theme::card),
    ]
    .spacing(6)
    .into()
}

fn assistant<'a>(app: &'a App, entry: &'a Entry, reply: &'a Reply) -> Element<'a, Message> {
    let mut header = row![text("AURA")
        .font(theme::ui_semibold())
        .size(12)
        .color(theme::BLUE)]
    .spacing(8)
    .align_y(Alignment::Center);
    if reply.streaming {
        header = header.push(spinner(app, 11.0, theme::BLUE));
    }
    if let Some(model) = &reply.model {
        header = header.push(muted(model.as_str()).size(11));
    }
    if let Some(usage) = reply.usage {
        let mut used = format!("{} tok", usage.tokens);
        if let Some(cost) = usage.cost_usd {
            used.push_str(&format!(" · ${cost:.4}"));
        }
        header = header.push(text(used).size(11).color(theme::FAINT));
    }
    header = header.push(space::horizontal());
    if !reply.streaming && !reply.raw.is_empty() {
        header = header.push(
            button(muted("Copy").size(11))
                .padding([2, 6])
                .style(theme::ghost)
                .on_press(Message::Copy(reply.raw.clone())),
        );
    }
    header = header.push(timestamp(entry));

    let body: Element<'a, Message> = if reply.raw.is_empty() {
        muted("…").into()
    } else {
        markdown::view(reply.markdown.items(), theme::markdown_settings()).map(Message::LinkClicked)
    };
    column![header, body].spacing(8).into()
}

fn system<'a>(content: &'a str, level: Level) -> Element<'a, Message> {
    let color = match level {
        Level::Info => theme::MUTED,
        Level::Success => theme::CYAN,
        Level::Warning => theme::PURPLE,
        Level::Error => theme::RED,
    };
    text(content).size(12).color(color).into()
}

/// Clickable header row of a thinking block or tool call.
fn step_header<'a>(
    entry: &Entry,
    icon: Element<'a, Message>,
    title: String,
    details: Vec<String>,
) -> Element<'a, Message> {
    let mut line = row![
        container(icon).width(16).center_x(16),
        text(title)
            .size(13)
            .color(theme::TEXT)
            .wrapping(Wrapping::None),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    for detail in details {
        line = line.push(
            text(detail)
                .size(11)
                .color(theme::FAINT)
                .wrapping(Wrapping::None),
        );
    }
    line = line.push(space::horizontal());
    line = line.push(
        text(if entry.expanded { "▾" } else { "▸" })
            .size(11)
            .color(theme::FAINT),
    );
    button(line)
        .width(Fill)
        .padding([4, 8])
        .style(theme::ghost)
        .on_press(Message::ToggleEntry(entry.id))
        .into()
}

fn glyph<'a>(symbol: &'a str, color: Color) -> Element<'a, Message> {
    text(symbol).size(12).color(color).into()
}

fn thinking_view<'a>(
    app: &'a App,
    entry: &'a Entry,
    thinking: &'a Thinking,
) -> Element<'a, Message> {
    let icon = if thinking.running {
        spinner(app, 12.0, theme::PURPLE)
    } else {
        glyph("◇", theme::PURPLE)
    };
    let title = if thinking.running {
        "Thinking".to_string()
    } else {
        match thinking.duration_ms {
            Some(ms) => format!("Thought for {}", duration(ms)),
            None => "Thought".to_string(),
        }
    };
    let header = step_header(entry, icon, title, Vec::new());
    if !entry.expanded || thinking.text.is_empty() {
        return header;
    }
    column![
        header,
        container(text(&thinking.text).size(13).color(theme::MUTED))
            .padding(super::pad(2.0, 0.0, 6.0, 32.0)),
    ]
    .into()
}

fn tool_view<'a>(app: &'a App, entry: &'a Entry, tool: &'a Tool) -> Element<'a, Message> {
    let icon = match tool.status {
        ToolStatus::Running => spinner(app, 12.0, theme::BLUE),
        ToolStatus::Success => glyph("✓", theme::CYAN),
        ToolStatus::Error => glyph("✕", theme::RED),
        ToolStatus::Interrupted => glyph("■", theme::MUTED),
    };
    let label = format_tool_label(&tool.name, &tool.args);
    let title = if label.is_empty() {
        tool.name.clone()
    } else {
        label
    };
    let mut details = Vec::new();
    if let Some((attempt, max)) = tool.retry {
        details.push(format!("retry {attempt}/{max}"));
    }
    if let Some(ms) = tool.duration_ms {
        details.push(duration(ms));
    }
    if tool.status == ToolStatus::Interrupted {
        details.push("interrupted".to_string());
    }
    if let Some(policy) = &tool.policy {
        details.push(policy.clone());
    }
    let header = step_header(entry, icon, title, details);
    if !entry.expanded {
        return header;
    }

    let args = serde_json::to_string_pretty(&parse_tool_args(&tool.args))
        .unwrap_or_else(|_| tool.args.clone());
    let mut detail = column![
        text(&tool.name).size(11).color(theme::FAINT),
        code_box(
            super::mono_line(args)
                .wrapping(Wrapping::WordOrGlyph)
                .into()
        ),
    ]
    .spacing(6);
    if let Some(result) = &tool.result {
        detail = detail.push(text("result").size(11).color(theme::FAINT));
        detail = detail.push(code_box(output(&tool.name, result)));
    }
    column![
        header,
        container(detail).padding(super::pad(4.0, 0.0, 8.0, 32.0))
    ]
    .into()
}

fn shell_view<'a>(app: &'a App, shell: &'a Shell) -> Element<'a, Message> {
    let mut prompt = row![
        text("$")
            .font(theme::mono_medium())
            .size(theme::MONO_SIZE)
            .color(theme::CYAN),
        text(&shell.command)
            .font(theme::mono_medium())
            .size(theme::MONO_SIZE)
            .color(theme::WHITE),
        space::horizontal(),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    match (&shell.output, shell.exit_code) {
        (None, _) => prompt = prompt.push(spinner(app, 12.0, theme::CYAN)),
        (Some(_), Some(code)) => {
            prompt = prompt.push(
                container(text(format!("exit {code}")).size(11))
                    .padding([1, 6])
                    .style(theme::badge(theme::RED)),
            );
        }
        (Some(out), None) if !out.is_empty() => {
            prompt = prompt.push(
                button(muted("Copy").size(11))
                    .padding([2, 6])
                    .style(theme::ghost)
                    .on_press(Message::Copy(crate::ansi::strip(out))),
            );
        }
        _ => {}
    }
    let mut body = column![prompt].spacing(8);
    if let Some(out) = shell.output.as_deref().filter(|o| !o.is_empty()) {
        let (shown, hidden) = clip_lines(out, MAX_OUTPUT_LINES);
        body = body.push(ansi_text(shown));
        if hidden > 0 {
            body = body.push(muted(format!("… {hidden} more lines")));
        }
    }
    code_box(body.into())
}

fn code_box(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .padding([8, 12])
        .width(Fill)
        .style(theme::code)
        .into()
}

/// A tool result: command output keeps its colors, diffs get colored
/// gutters, anything else is plain mono text.
fn output<'a>(tool: &str, result: &'a str) -> Element<'a, Message> {
    let (shown, hidden) = clip_lines(result, MAX_OUTPUT_LINES);
    let body: Element<'a, Message> = if is_diff(shown) {
        diff(shown)
    } else if tool == "run_command" || shown.contains('\x1b') {
        ansi_text(shown)
    } else {
        super::mono_line(shown)
            .wrapping(Wrapping::WordOrGlyph)
            .into()
    };
    if hidden == 0 {
        return body;
    }
    column![body, muted(format!("… {hidden} more lines"))]
        .spacing(4)
        .into()
}

fn is_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("@@ ")) || {
        let mut lines = text.lines();
        lines.any(|l| l.starts_with("--- ")) && lines.any(|l| l.starts_with("+++ "))
    }
}

fn diff(patch: &str) -> Element<'_, Message> {
    let lines = patch.lines().map(|line| {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            theme::MUTED
        } else if line.starts_with('+') {
            theme::CYAN
        } else if line.starts_with('-') {
            theme::RED
        } else if line.starts_with("@@") {
            theme::PURPLE
        } else {
            Color::TRANSPARENT
        };
        let changed = color != Color::TRANSPARENT && color != theme::MUTED;
        row![
            container(text(""))
                .width(3)
                .height(DIFF_LINE_HEIGHT)
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    ..container::Style::default()
                }),
            container(super::mono_line(line).color(if changed { color } else { theme::TEXT }))
                .padding([0, 8])
                .width(Fill)
                .height(DIFF_LINE_HEIGHT)
                .center_y(DIFF_LINE_HEIGHT)
                .style(theme::diff_line(if changed {
                    color
                } else {
                    Color::TRANSPARENT
                })),
        ]
        .into()
    });
    Column::with_children(lines).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unified_diffs() {
        assert!(is_diff("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b"));
        assert!(is_diff("@@ -1,2 +1,2 @@\n x"));
        assert!(!is_diff("- a list\n- of items"));
    }
}
