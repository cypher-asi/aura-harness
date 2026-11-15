//! Run graph over the chat panel: the collapsible run → turn →
//! iteration → step tree on the left, the selected node's metadata and
//! links on the right. Nodes linked to the selection are marked in the
//! tree.

use super::panels::{render_scrollbar, truncate_record_info, ScrollView};
use super::text::{display_width, wrap_words};
use crate::app::graph::GraphRow;
use crate::events::{TraceNode, TraceNodeStatus};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

const HINT: &str = "↑↓ move  ←→ fold  Tab follow link  e issues  Esc close";

/// Narrowest chat panel that gets a side-by-side metadata pane.
const SPLIT_MIN_WIDTH: u16 = 70;

pub(super) const fn status_glyph(status: TraceNodeStatus, theme: &Theme) -> (&'static str, Color) {
    match status {
        TraceNodeStatus::Pending => ("●", theme.colors.pending),
        TraceNodeStatus::Ok => ("✓", theme.colors.success),
        TraceNodeStatus::Warn => ("!", theme.colors.warning),
        TraceNodeStatus::Error => ("✗", theme.colors.error),
    }
}

fn kind_color(kind: &str, theme: &Theme) -> Color {
    match kind {
        "run" | "turn" => theme.colors.primary,
        "decision" => theme.colors.warning,
        "reward" => theme.colors.success,
        "subagent" | "branch" => theme.colors.secondary,
        _ => theme.colors.foreground,
    }
}

fn tree_prefix(row: &GraphRow) -> String {
    let mut out = String::new();
    for (depth, &more) in row.guides.iter().enumerate() {
        if depth == 0 {
            continue;
        }
        out.push_str(if more { "│ " } else { "  " });
    }
    if !row.guides.is_empty() {
        out.push_str(if row.last { "└─" } else { "├─" });
    }
    out.push_str(match (row.has_children, row.collapsed) {
        (true, true) => "▸ ",
        (true, false) => "▾ ",
        (false, _) => "  ",
    });
    out
}

fn tree_line<'a>(
    row: &GraphRow,
    node: &TraceNode,
    selected: bool,
    linked: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let (glyph, color) = status_glyph(node.status, theme);
    let prefix = tree_prefix(row);
    let marker = if linked { "↔ " } else { "" };
    let used = display_width(&prefix) + 2 + display_width(marker);
    let label = truncate_record_info(&node.label, width.saturating_sub(used).max(1));
    let rest = width.saturating_sub(used + display_width(&label) + 1);
    let detail = if node.detail.is_empty() || rest < 4 {
        String::new()
    } else {
        format!(" {}", truncate_record_info(&node.detail, rest))
    };
    let muted = theme.colors.muted;
    let mut spans = vec![
        Span::styled(prefix, Style::default().fg(muted)),
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
    ];
    if linked {
        spans.push(Span::styled(
            marker,
            Style::default().fg(theme.colors.secondary),
        ));
    }
    let label_style = Style::default().fg(if linked {
        theme.colors.secondary
    } else {
        kind_color(&node.kind, theme)
    });
    spans.push(Span::styled(label, label_style));
    spans.push(Span::styled(detail, Style::default().fg(muted)));
    let line = Line::from(spans);
    if selected {
        line.style(
            Style::default()
                .bg(theme.colors.selection)
                .fg(theme.colors.on_selection)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        line
    }
}

/// Metadata and links of `node`, wrapped to `width`.
fn meta_lines<'a>(app: &App, node: &TraceNode, width: usize, theme: &Theme) -> Vec<Line<'a>> {
    let muted = Style::default().fg(theme.colors.muted);
    let fg = Style::default().fg(theme.colors.foreground);
    let (glyph, color) = status_glyph(node.status, theme);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            Span::styled(
                node.label.clone(),
                Style::default()
                    .fg(kind_color(&node.kind, theme))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            match node.seq {
                Some(seq) => format!("{} · {} · step {seq}", node.kind, node.id),
                None => format!("{} · {}", node.kind, node.id),
            },
            muted,
        )),
    ];
    if !node.detail.is_empty() {
        for wrapped in wrap_words(&node.detail, width.max(1)) {
            lines.push(Line::from(Span::styled(wrapped, fg)));
        }
    }
    let key_width = node
        .meta
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0)
        .min(16);
    if !node.meta.is_empty() {
        lines.push(Line::from(""));
    }
    for (key, value) in &node.meta {
        let room = width.saturating_sub(key_width + 1).max(8);
        let mut first = true;
        for raw in value.lines() {
            for wrapped in wrap_words(raw, room) {
                let name = if first { key.as_str() } else { "" };
                first = false;
                lines.push(Line::from(vec![
                    Span::styled(format!("{name:<key_width$} "), muted),
                    Span::styled(wrapped, fg),
                ]));
            }
        }
    }
    let links = app.graph().links_of(&node.id);
    if !links.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("── links ──", muted)));
        for (kind, other) in links {
            let label = app
                .graph()
                .node(other)
                .map_or_else(|| other.clone(), |n| n.label.clone());
            lines.push(Line::from(vec![
                Span::styled(format!("{kind:<12} "), muted),
                Span::styled(
                    truncate_record_info(&label, width.saturating_sub(13).max(4)),
                    Style::default().fg(theme.colors.secondary),
                ),
            ]));
        }
    }
    lines
}

/// Draw the graph over `area`, the chat panel.
pub(super) fn render_graph(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let rows = app.graph().rows();
    let title = format!(
        " Run graph {}{} ",
        app.graph()
            .run_id()
            .get(..13)
            .unwrap_or(app.graph().run_id()),
        if app.graph().issues_only() {
            " · issues"
        } else {
            ""
        }
    );
    let footer = Line::from(Span::styled(
        format!(
            " {} ",
            truncate_record_info(HINT, usize::from(area.width).saturating_sub(4))
        ),
        Style::default().fg(theme.colors.muted),
    ));
    let block = Block::default()
        .title(title)
        .title_bottom(footer)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let split = inner.width >= SPLIT_MIN_WIDTH;
    let tree_width = if split {
        inner.width * 3 / 5
    } else {
        inner.width
    };
    let meta_height = if split { 0 } else { (inner.height / 3).min(10) };
    let tree = Rect {
        width: tree_width,
        height: inner.height.saturating_sub(meta_height),
        ..inner
    };
    let meta = if split {
        Rect {
            x: inner.x + tree_width + 1,
            width: inner.width.saturating_sub(tree_width + 1),
            ..inner
        }
    } else {
        Rect {
            y: inner.y + tree.height,
            height: meta_height,
            ..inner
        }
    };

    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No steps recorded yet.",
                Style::default().fg(theme.colors.muted),
            ))),
            tree,
        );
        app.set_detail_bounds(0, 0);
        return;
    }

    let visible = usize::from(tree.height);
    let first = app.graph_mut().sync_scroll(&rows, visible);
    let selected = app.graph().selected_row(&rows);
    let linked = app.graph().linked_to_selected();
    let width = usize::from(tree.width);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(i, row)| {
            let node = &app.graph().nodes()[row.node];
            tree_line(
                row,
                node,
                i == selected,
                linked.contains(node.id.as_str()),
                width,
                theme,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), tree);
    render_scrollbar(
        frame,
        area,
        tree,
        ScrollView {
            total: rows.len(),
            visible,
            first,
        },
        true,
        theme,
    );

    if let Some(node) = app.graph().selected_node().cloned() {
        if split {
            let divider: Vec<Line> = (0..meta.height)
                .map(|_| Line::from(Span::styled("│", Style::default().fg(theme.colors.muted))))
                .collect();
            frame.render_widget(
                Paragraph::new(divider),
                Rect {
                    x: meta.x.saturating_sub(1),
                    width: 1,
                    ..meta
                },
            );
        }
        let pane = Rect {
            x: meta.x.saturating_add(1),
            width: meta.width.saturating_sub(1),
            ..meta
        };
        let lines = meta_lines(app, &node, usize::from(pane.width), theme);
        frame.render_widget(Paragraph::new(lines), pane);
    }
    app.set_detail_bounds(0, visible);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;
    use ratatui::{backend::TestBackend, Terminal};

    fn node(id: &str, parent: Option<&str>, label: &str) -> TraceNode {
        TraceNode {
            id: id.into(),
            kind: if parent.is_none() { "run" } else { "step" }.into(),
            parent: parent.map(str::to_string),
            label: label.into(),
            status: TraceNodeStatus::Ok,
            ..TraceNode::default()
        }
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::cyber();
        terminal
            .draw(|frame| render_graph(frame, frame.area(), app, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn draws_tree_metadata_and_links() {
        let mut app = App::new();
        let mut nudge = node("s2", Some("t1"), "steering/implement_now");
        nudge.status = TraceNodeStatus::Warn;
        nudge.links = vec![("triggered".into(), "s3".into())];
        let mut model = node("s3", Some("t1"), "model → write_file");
        model.meta = vec![("tokens".into(), "1.2k in".into())];
        app.process_command(UiCommand::GraphLoad {
            run_id: "0193abcd-0000-7000".into(),
            nodes: vec![
                node("run", None, "run test-model"),
                node("t1", Some("run"), "turn 1: add a greeting"),
                nudge,
                model,
            ],
        });
        let text = screen(&mut app, 100, 14);
        assert!(text.contains("Run graph 0193abcd-0000"), "{text}");
        assert!(text.contains("▾ ✓ turn 1: add a greeting"), "{text}");
        assert!(text.contains("├─  ! ↔ steering/implement_now"), "{text}");
        assert!(text.contains("└─  ✓ model → write_file"), "{text}");
        assert!(text.contains("tokens 1.2k in"), "{text}");
        assert!(text.contains("←triggered"), "{text}");
    }

    #[test]
    fn narrow_panel_stacks_metadata_below() {
        let mut app = App::new();
        app.process_command(UiCommand::GraphLoad {
            run_id: "r".into(),
            nodes: vec![
                node("run", None, "run m"),
                node("t1", Some("run"), "turn 1"),
            ],
        });
        let text = screen(&mut app, 50, 16);
        assert!(text.contains("turn 1"), "{text}");
        assert!(text.contains("step · t1"), "{text}");
    }
}
