//! The help overlay: the commands and keys.

use super::super::text::centered_rect;
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// The help overlay's text, one entry a line.
const HELP_ITEMS: [&str; 48] = [
    "/help      Show this help",
    "/new       New session (reset context)",
    "/resume [n] List or resume earlier sessions",
    "/rewind [n] [code|chat]  Undo to before turn n",
    "/diff [session]     Files changed by last turn",
    "/gate /compare      Gate's table, A/B run",
    "/clear     Clear messages",
    "/context   What fills the context window",
    "/compact [focus]    Summarize older history now",
    "/init      Write or improve AGENTS.md",
    "/todos     Fold or open the to-do drawer",
    "/cost      Tokens and cost of the session",
    "/agent [n|name]     Add an agent, or switch",
    "/agent new [name]   Create and switch to agent",
    "/project   The current project's folder, agents",
    "/archive /remove-project  Hide agent, project",
    "/agents /kernel /files  Toggle those panels",
    "/graph [id] /runs /status  Run graphs, scores",
    "/model [opus] /thinking [level]  Model, thinking",
    "/mode [name] /approval [level]  Work, what asks",
    "/plan /build [path]  Plan mode, implement plan",
    "/login /whoami  Login for cloud models, status",
    "/login claude|chatgpt [paste]  Link a plan",
    "/logout [claude|chatgpt]  Sign out / unlink",
    "/quit      Exit",
    "",
    "Enter / Shift+Enter  Send / new line",
    "@path      Attach a file or folder",
    "Ctrl+V     Paste an image (or text)",
    "Ctrl+G     Edit the input in $EDITOR",
    "$ / Esc    Open the terminal / back to chat",
    "Tab        Switch panels",
    "Shift+Tab  Approval level (yolo→ask→strict)",
    "↑/↓        Navigate / History",
    "Ctrl+N     New project (pick its folder)",
    "Space/Shift+Space  Fold agent / its project",
    "Shift+←/→  Panel tabs (chat: TERMINAL, GRAPH)",
    "Ctrl+X /expand  Chat panel full screen",
    "o / k / l  Process: open agent, kill, log",
    "Alt+↑/↓/1..5 Alt+T  Agent, app, thinking level",
    "Mouse      Wheel/click agents to switch",
    "PgUp/PgDn Ctrl+Home/End  Scroll chat, to end",
    "Ctrl+O     Expand/collapse thinking & tools",
    "Click      Open a summary, then a step's details",
    "Shift+Mouse Select text to copy",
    "Ctrl+C/T   Cancel or exit / stop the command",
    "",
    "Press any key to close",
];

/// Render the help overlay.
pub(super) fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let help_text = build_help_lines(theme);
    // Every line plus the two border rows, so nothing is clipped on a
    // screen tall enough to hold it.
    let modal_height = u16::try_from(help_text.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

fn build_help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let fg = theme.colors.foreground;
    HELP_ITEMS
        .into_iter()
        .map(|s| {
            if s.is_empty() {
                Line::from("")
            } else if s == "Press any key to close" {
                Line::from(Span::styled(s, Style::default().fg(theme.colors.muted)))
            } else {
                Line::from(Span::styled(s, Style::default().fg(fg)))
            }
        })
        .collect()
}
