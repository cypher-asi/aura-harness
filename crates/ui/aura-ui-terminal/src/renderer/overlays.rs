//! Overlay rendering: approval modal, help, and the lines of a record's
//! details.

use super::markdown::{parse_content_segments, render_markdown, ContentSegment};
use super::text::{centered_rect, display_width, wrap_words};
use crate::{
    app::{folder_picker::FolderRow, AppState},
    components::{highlight_code_line, CodeBlock, Info, Value},
    App, Theme,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::path::Path;

/// Render overlay elements (modals, help).
pub(super) fn render_overlays(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if let Some(approval) = app.pending_approval() {
        render_approval_modal(frame, approval, app.pending_approval_count(), theme);
    }

    if app.state() == AppState::ShowingHelp {
        render_help_overlay(frame, theme);
    }

    if app.state() == AppState::SelectingModel {
        render_model_selector(frame, app, theme);
    }

    if let Some(project) = app.project_info() {
        render_project_info(frame, project, theme);
    }
}

/// Render the approval modal.
fn render_approval_modal(
    frame: &mut Frame,
    approval: &crate::app::PendingApproval,
    waiting: usize,
    theme: &Theme,
) {
    /// Most lines of the action's description shown.
    const MAX_DESCRIPTION_LINES: usize = 6;

    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let text_width = usize::from(modal_width.saturating_sub(2)).max(1);
    let muted = Style::default().fg(theme.colors.muted);

    let mut content = vec![Line::from(Span::styled(
        format!("{} wants to:", approval.tool),
        Style::default().fg(theme.colors.foreground),
    ))];
    let description = wrap_words(&approval.description, text_width);
    let cut = description.len() > MAX_DESCRIPTION_LINES;
    content.extend(
        description
            .into_iter()
            .take(MAX_DESCRIPTION_LINES)
            .map(|line| Line::from(Span::styled(line, muted))),
    );
    if cut {
        content.push(Line::from(Span::styled("…", muted)));
    }
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[Y] Allow once",
        Style::default().fg(theme.colors.success),
    )));
    if !approval.always.is_empty() {
        content.push(Line::from(Span::styled(
            format!("[A] Always allow {}", approval.always),
            Style::default().fg(theme.colors.primary),
        )));
    }
    content.push(Line::from(Span::styled(
        "[N] Deny (Esc)",
        Style::default().fg(theme.colors.error),
    )));

    let height = u16::try_from(content.len() + 2).unwrap_or(u16::MAX);
    let modal_area = centered_rect(modal_width, height.min(area.height), area);
    frame.render_widget(Clear, modal_area);

    let title = if waiting > 1 {
        format!(" Approval Required (1 of {waiting}) ")
    } else {
        " Approval Required ".to_string()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.warning));
    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Rows of the model selector that are not the list: the tab row, the
/// filter row, the blank lines around the list, the key hints and the
/// two borders.
const MODEL_SELECTOR_CHROME_ROWS: usize = 7;

/// Render the model selector: a Ready / Sign-in tab row, the filter, and
/// the models on the tab grouped by route, scrolled to keep the cursor in
/// view.
fn render_model_selector(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let picker = app.model_picker();
    let models = picker.visible(app.models());
    let muted = Style::default().fg(theme.colors.muted);

    let (ready, sign_in) = picker.counts(app.models());
    let tab = |label: &str, count: usize, active: bool| {
        let style = if active {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            muted
        };
        Span::styled(format!("{label} ({count})"), style)
    };
    let showing_ready = picker.tab() == crate::app::ModelTab::Ready;
    let tabs = Line::from(vec![
        tab("Ready", ready, showing_ready),
        Span::raw("  "),
        tab("Sign in", sign_in, !showing_ready),
    ]);
    let filter = if picker.filter().is_empty() {
        Line::from(Span::styled("Type to filter", muted))
    } else {
        Line::from(Span::styled(
            format!("/ {}█", picker.filter()),
            Style::default().fg(theme.colors.foreground),
        ))
    };

    // The list, each row with the index of its model (headings and
    // separators have none), so the window can follow the cursor.
    let mut rows: Vec<(Line<'_>, Option<usize>)> = Vec::new();
    let mut last_group: Option<&str> = None;
    for (idx, model) in models.iter().enumerate() {
        if last_group != Some(model.group.as_str()) {
            if last_group.is_some() {
                rows.push((Line::from(""), None));
            }
            rows.push((Line::from(Span::styled(model.group.as_str(), muted)), None));
            last_group = Some(model.group.as_str());
        }
        rows.push((
            model_row(app, model, idx == picker.cursor(), theme),
            Some(idx),
        ));
    }
    if rows.is_empty() {
        let empty = if picker.filter().is_empty() && !showing_ready {
            "Everything is signed in"
        } else {
            "No models match"
        };
        rows.push((Line::from(Span::styled(empty, muted)), None));
    }

    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let max_height = usize::from(area.height.saturating_sub(2));
    let room = max_height
        .saturating_sub(MODEL_SELECTOR_CHROME_ROWS)
        .max(1)
        .min(rows.len());
    let cursor_row = rows
        .iter()
        .position(|(_, idx)| *idx == Some(picker.cursor()))
        .unwrap_or(0);
    let start = list_window_start(rows.len(), room, cursor_row);
    let total = rows.len();
    // Markers need a row each beside the cursor's; in a window too small
    // for that, every row is a list row.
    let markers = room >= 3;

    let mut lines = vec![tabs, filter, Line::from("")];
    // The line of the selector each model row is on, with its index.
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (offset, (line, idx)) in rows.into_iter().skip(start).take(room).enumerate() {
        if markers && offset == 0 && start > 0 {
            lines.push(Line::from(Span::styled("  ↑ more", muted)));
        } else if markers && offset + 1 == room && start + room < total {
            lines.push(Line::from(Span::styled("  ↓ more", muted)));
        } else {
            if let Some(idx) = idx {
                hits.push((lines.len(), idx));
            }
            lines.push(line);
        }
    }
    lines.push(Line::from(""));
    let hints = if modal_width >= 62 {
        "↑/↓ move  Tab switch  type to filter  Enter select  Esc close"
    } else {
        "↑/↓ Tab type Enter Esc"
    };
    lines.push(Line::from(Span::styled(hints, muted)));

    let modal_height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let title = match picker.target() {
        crate::app::PickerTarget::Session => " Select model ".to_string(),
        crate::app::PickerTarget::BenchDifficulty { arm, difficulty } => format!(
            " Model for {} work · arm {} ",
            crate::app::DIFFICULTIES
                .get(difficulty)
                .copied()
                .unwrap_or("medium"),
            crate::events::arm_letter(arm)
        ),
        crate::app::PickerTarget::BenchClassifier => {
            " Model that classifies the prompts ".to_string()
        }
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(muted);
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);

    // Lines past the bottom border are cut, so only rows inside it click.
    let shown = usize::from(modal_height.saturating_sub(2));
    let rows = hits
        .into_iter()
        .filter(|(line, _)| *line < shown)
        .filter_map(|(line, idx)| {
            let y = modal_area
                .y
                .checked_add(1)?
                .checked_add(u16::try_from(line).ok()?)?;
            Some((y, idx))
        })
        .collect();
    app.set_model_picker_layout(crate::app::PickerLayout {
        modal: modal_area,
        rows,
    });
}

/// First row of a `room`-row window over `total` rows that shows
/// `cursor`: the earliest window that has it, with one row below it to
/// spare, since the window's last row turns into a `↓ more` marker when
/// rows are hidden past it. The selector keeps no scroll state, so the
/// window is worked out again on every frame from the cursor alone.
fn list_window_start(total: usize, room: usize, cursor: usize) -> usize {
    if total <= room {
        return 0;
    }
    if room < 3 {
        // No room for markers around the cursor: put it first.
        return cursor.min(total - room);
    }
    (cursor + 2).saturating_sub(room).min(total - room)
}

/// One model's row: cursor marker, current check, label and what it
/// still needs (`not pulled`, `/login <provider>`).
fn model_row<'a>(
    app: &App,
    model: &'a crate::events::ModelOptionView,
    selected: bool,
    theme: &Theme,
) -> Line<'a> {
    let current = model.id == app.picker_marked();
    let marker = if selected { "› " } else { "  " };
    let check = if current { "● " } else { "  " };
    let note = if model.local && !model.installed {
        "  not pulled".to_string()
    } else if model.available {
        String::new()
    } else {
        match model.provider.as_deref() {
            Some(provider) if provider != "zos" => format!("  /login {provider}"),
            _ => "  login required".to_string(),
        }
    };
    let label_color = if selected {
        theme.colors.primary
    } else {
        theme.colors.foreground
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.colors.primary)),
        Span::styled(check, Style::default().fg(theme.colors.success)),
        Span::styled(model.label.as_str(), Style::default().fg(label_color)),
        Span::styled(note, Style::default().fg(theme.colors.warning)),
    ])
}

/// Render the new-project folder browser: the directory being browsed,
/// its subfolders, and the keys that drive it.
pub(super) fn render_folder_picker(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if app.state() != AppState::SelectingFolder {
        return;
    }
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 24.min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    let inner_width = usize::from(modal_width.saturating_sub(2));
    // Path line, blank line above the footer, and two footer lines.
    let fixed_rows = 4 + usize::from(
        app.folder_picker()
            .is_some_and(|p| p.new_folder().is_some()),
    );
    let visible = usize::from(modal_height.saturating_sub(2)).saturating_sub(fixed_rows);
    let scroll = app.sync_folder_picker_scroll(visible);
    let Some(picker) = app.folder_picker() else {
        return;
    };

    let muted = Style::default().fg(theme.colors.muted);
    let path = tilde_path(picker.dir());
    let mut lines = vec![Line::from(Span::styled(
        truncate_start(&path, inner_width),
        Style::default().fg(theme.colors.secondary),
    ))];

    for (index, row) in picker.rows().iter().enumerate().skip(scroll).take(visible) {
        let selected = index == picker.selected();
        let marker = Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(theme.colors.primary),
        );
        let label_style = |color| {
            Style::default().fg(if selected {
                theme.colors.primary
            } else {
                color
            })
        };
        let line = match row {
            FolderRow::UseThis => Line::from(vec![
                marker,
                Span::styled("✓ ", Style::default().fg(theme.colors.success)),
                Span::styled("Use this folder", label_style(theme.colors.success)),
            ]),
            FolderRow::Parent => Line::from(vec![
                marker,
                Span::styled("  ", muted),
                Span::styled("..", label_style(theme.colors.foreground)),
            ]),
            FolderRow::Folder(name) => Line::from(vec![
                marker,
                Span::styled("▰ ", muted),
                Span::styled(name.as_str(), label_style(theme.colors.foreground)),
            ]),
            FolderRow::More(hidden) => Line::from(vec![
                marker,
                Span::styled(format!("  … {hidden} more"), muted),
            ]),
        };
        lines.push(line);
    }
    let shown = lines.len();
    lines.extend(std::iter::repeat_n(
        Line::from(""),
        (visible + 1).saturating_sub(shown),
    ));

    if let Some(name) = picker.new_folder() {
        lines.push(Line::from(vec![
            Span::styled("New folder: ", Style::default().fg(theme.colors.primary)),
            Span::styled(name, Style::default().fg(theme.colors.foreground)),
            Span::styled("█", Style::default().fg(theme.colors.primary)),
        ]));
    }
    lines.push(Line::from(""));
    if picker.new_folder().is_some() {
        lines.push(Line::from(Span::styled("Enter create  Esc stop", muted)));
        lines.push(Line::from(""));
    } else {
        let hidden = if picker.show_hidden() { "hide" } else { "show" };
        lines.push(Line::from(Span::styled(
            "↑/↓ move  Enter/→ open  ←/Backspace up  Space pick highlighted",
            muted,
        )));
        lines.push(Line::from(Span::styled(
            format!("n new folder  ~ home  . {hidden} hidden  Esc cancel"),
            muted,
        )));
    }

    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(" Select project folder ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// `path` with the home directory shown as `~`.
fn tilde_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `text` cut from the left with `…` so it fits `width` columns.
fn truncate_start(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = display_width(c.encode_utf8(&mut [0; 4]));
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// Render a project's details: its name, folder, when it was made, and
/// how many of its agents are listed and archived.
fn render_project_info(frame: &mut Frame, project: &crate::events::ProjectSummary, theme: &Theme) {
    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let mut info = Info::new(usize::from(modal_width.saturating_sub(2)).max(1), theme);
    info.section("FOLDER", None);
    info.text(&project.folder, Value::Plain);
    if !project.folder_exists {
        info.text("The folder is missing", Value::Flag);
    }
    info.section("PROJECT", None);
    info.fact("Created", &project.created, Value::Plain);
    info.fact("Agents", &project.listed_agents.to_string(), Value::Plain);
    info.fact("Archived", &project.archived_agents.to_string(), Value::Plain);
    info.blank();
    info.note("Press any key to close");
    let lines = info.into_lines();

    let modal_height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);
    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(format!(" {} ", project.name.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        modal_area,
    );
}

/// Render the help overlay.
fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
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
    let items = [
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
        "/todos     Toggle the agent's todo list",
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
        "Alt+↑/↓/1..4 Alt+T  Agent, app, thinking level",
        "Mouse      Wheel/click agents to switch",
        "PgUp/PgDn Ctrl+Home/End  Scroll chat, to end",
        "Ctrl+O     Expand/collapse thinking & tools",
        "Click      Open a summary, then a step's details",
        "Shift+Mouse Select text to copy",
        "Ctrl+C/T   Cancel or exit / stop the command",
        "",
        "Press any key to close",
    ];

    items
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

/// Status glyph and color shown in a record's title.
pub(super) const fn record_status(
    record: &crate::events::RecordSummary,
    theme: &Theme,
) -> (&'static str, Color) {
    use crate::events::RecordStatus;
    match record.status {
        RecordStatus::Ok => ("✓", theme.colors.success),
        RecordStatus::Error => ("✗", theme.colors.error),
        RecordStatus::Pending => ("◌", theme.colors.pending),
        RecordStatus::None => ("·", theme.colors.muted),
    }
}

/// Columns of a label in the RECORD and PROCESS views, whose labels are
/// longer than a panel's.
pub(super) const DETAIL_LABEL_WIDTH: usize = 14;

/// Every line of a record's details, laid out `width` columns wide.
pub(super) fn record_detail_lines(
    record: &crate::events::RecordSummary,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme)
        .label_width(DETAIL_LABEL_WIDTH)
        .wrap_values();
    info.section("RECORD", None);
    info.fact("Sequence", &record.seq.to_string(), Value::Plain);
    info.fact("Context hash", &record.full_hash, Value::Quiet);

    info.section("TRANSACTION", None);
    info.fact("Tx id", &record.tx_id, Value::Quiet);
    info.fact("Agent id", &record.agent_id, Value::Quiet);
    let time = format!("{} ({})", record.ts_ms, record.timestamp);
    info.fact("Time", &time, Value::Plain);
    info.fact("Kind", &record.tx_kind, Value::Plain);
    if !record.info.is_empty() {
        info.fact("Info", &record.info, Value::Plain);
    }

    info.section("PROCESSING", None);
    info.fact("Sender", &record.sender, Value::Plain);
    info.fact("Actions", &record.action_count.to_string(), Value::Plain);
    info.fact("Effects", &record.effect_status, Value::Plain);

    if let Some(policy) = &record.policy {
        policy_facts(&mut info, policy);
    }
    if !record.error_details.is_empty() {
        info.section("ERROR", None);
        info.text(&record.error_details, Value::Error);
    }
    info.section("MESSAGE", None);
    let message = message_lines(&record.message, info.indented_width(), theme);
    info.indented(message);
    info.into_lines()
}

/// What the kernel decided about a tool, by which rule, and the checks it
/// went through to get there.
fn policy_facts(info: &mut Info, policy: &crate::events::PolicyDetail) {
    let verdict = match policy.verdict.as_str() {
        "approved" => Value::Plain,
        "denied" => Value::Error,
        _ => Value::Flag,
    };
    info.section("POLICY DECISION", None);
    info.fact("Tool", &policy.tool, Value::Plain);
    info.fact("Verdict", &policy.verdict, verdict);
    info.fact("Decided by", &policy.decided_by, Value::Plain);
    if !policy.reason.is_empty() {
        info.fact("Reason", &policy.reason, Value::Plain);
    }
    if !policy.approval.is_empty() {
        info.fact("Approved by", &policy.approval, Value::Plain);
    }
    info.fact("Mode", &policy.mode, Value::Plain);
    info.fact("Proposal tx", &policy.proposal_tx, Value::Quiet);
    info.fact("Policy hash", &policy.policy_hash, Value::Quiet);
    for (index, check) in policy.trace.iter().enumerate() {
        let label = if index == 0 { "Checks" } else { "" };
        info.fact(label, check, Value::Plain);
    }
}

/// A record's message laid out `width` columns wide: JSON highlighted,
/// anything else as markdown with its code blocks.
fn message_lines(message: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if message.starts_with(['{', '[']) {
        // Highlight the whole document in order so keys on later lines are
        // recognized as keys (inside their object), not stray strings.
        return match crate::components::syntax::syntax_for_token("json") {
            Some(syntax) => {
                let mut highlighter = crate::components::syntax::LineHighlighter::new(syntax);
                message
                    .lines()
                    .map(|line| Line::from(highlighter.spans(line, theme)))
                    .collect()
            }
            None => message
                .lines()
                .map(|line| Line::from(highlight_code_line("json", line, theme)))
                .collect(),
        };
    }
    let base_style = Style::default().fg(theme.colors.foreground);
    let mut lines = Vec::new();
    for segment in parse_content_segments(message) {
        match segment {
            ContentSegment::Text(text) => {
                lines.extend(render_markdown(&text, width, base_style, theme));
            }
            ContentSegment::CodeBlock { language, code } => {
                lines.extend(CodeBlock::new(language, code).render(theme, width));
            }
        }
    }
    lines
}

#[cfg(test)]
mod window_tests {
    use super::list_window_start;

    /// Whatever the room, the cursor's row is in the window and is not a
    /// row a `↑ more` / `↓ more` marker takes.
    #[test]
    fn the_cursor_row_is_always_shown() {
        for total in 1..40 {
            for room in 1..=total {
                for cursor in 0..total {
                    let start = list_window_start(total, room, cursor);
                    assert!(start + room <= total, "{total} {room} {cursor}");
                    assert!(
                        (start..start + room).contains(&cursor),
                        "{total} {room} {cursor}: window {start}"
                    );
                    if room >= 3 {
                        let offset = cursor - start;
                        assert!(offset != 0 || start == 0, "{total} {room} {cursor}");
                        assert!(
                            offset + 1 != room || start + room == total,
                            "{total} {room} {cursor}"
                        );
                    }
                }
            }
        }
    }
}
