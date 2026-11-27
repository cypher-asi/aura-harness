//! Run graph over the chat panel: the collapsible run → turn →
//! iteration → step tree. Enter or a click opens the selected node's
//! metadata and links in a modal. Nodes linked to the selection are
//! marked in the tree.

use super::chrome::{body_block, pad_top, render_scrollbar, ScrollView};
use super::header::HeaderInfo;
use super::side_list::render_empty;
use super::text::display_width;
use crate::app::graph::GraphRow;
use crate::components::truncate_record_info;
use crate::events::{TraceNode, TraceNodeStatus};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

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

/// The chat panel's header while the graph is open: the run, and whether
/// only issues are listed.
pub(super) fn graph_header(app: &App, theme: &Theme) -> (String, Option<HeaderInfo>) {
    let run = app.graph().run_id();
    let title = format!("RUN GRAPH {}", run.get(..13).unwrap_or(run));
    let info = app.graph().issues_only().then(|| HeaderInfo {
        text: "ISSUES ONLY".to_string(),
        short: "ISSUES".to_string(),
        color: theme.colors.warning,
    });
    (title, info)
}

/// Draw the graph over `area`, the chat panel.
pub(super) fn render_graph(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let rows = app.graph().rows();
    let tree = render_frame(frame, area, app, theme);

    if rows.is_empty() {
        render_empty(frame, tree, "No steps recorded yet.", theme);
        app.graph_mut().set_tree_area(None);
        app.set_detail_bounds(0, 0);
        return;
    }

    let visible = usize::from(tree.height);
    let first = app.graph_mut().sync_scroll(&rows, visible);
    let lines = tree_lines(app, &rows, first, visible, usize::from(tree.width), theme);
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

    app.graph_mut().set_tree_area(Some(tree));
    app.set_detail_bounds(0, visible);
}

/// Draw the graph's frame over `area` and return where the tree's rows go
/// inside it, below its blank top row.
fn render_frame(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> Rect {
    let block = body_block((true, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    pad_top(inner)
}

/// The `visible` rows of the tree from `first`, `width` columns wide.
fn tree_lines<'a>(
    app: &App,
    rows: &[GraphRow],
    first: usize,
    visible: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let selected = app.graph().selected_row(rows);
    let linked = app.graph().linked_to_selected();
    rows.iter()
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::file_view::FileBody;
    use crate::app::DetailView;
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent};
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

    fn open_json(app: &App) -> String {
        match app.detail_view() {
            Some(DetailView::File(file)) => match &file.body {
                FileBody::Text(text) => {
                    assert_eq!(file.extension(), "json");
                    text.clone()
                }
                FileBody::Note(note) => panic!("note: {note}"),
            },
            other => panic!("not a file view: {other:?}"),
        }
    }

    #[test]
    fn enter_opens_the_step_as_json_and_esc_comes_back() {
        let mut app = App::new();
        let mut nudge = node("s2", Some("t1"), "steering/implement_now");
        nudge.status = TraceNodeStatus::Warn;
        nudge.links = vec![("triggered".into(), "s3".into())];
        let mut model = node("s3", Some("t1"), "model → write_file");
        model.meta = vec![
            ("tokens".into(), "1.2k in".into()),
            ("args".into(), r#"{"path":"src/main.rs"}"#.into()),
        ];
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
        let (title, info) = graph_header(&app, &Theme::cyber());
        assert_eq!(title, "RUN GRAPH 0193abcd-0000");
        assert!(info.is_none());
        assert!(text.contains("▾ ✓ turn 1: add a greeting"), "{text}");
        assert!(text.contains("├─  ! ↔ steering/implement_now"), "{text}");
        assert!(text.contains("└─  ✓ model → write_file"), "{text}");
        assert!(
            !text.contains("1.2k in"),
            "no preview beside the tree: {text}"
        );

        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let json = open_json(&app);
        assert!(json.contains("\"tokens\": \"1.2k in\""), "{json}");
        assert!(
            json.contains("\"args\": {\n      \"path\": \"src/main.rs\""),
            "{json}"
        );
        assert!(json.contains("\"kind\": \"←triggered\""), "{json}");

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.detail_view(), Some(&DetailView::Graph));
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.detail_view().is_none());
    }

    #[test]
    fn clicking_a_row_opens_it_full_panel() {
        let mut app = App::new();
        app.process_command(UiCommand::GraphLoad {
            run_id: "r".into(),
            nodes: vec![
                node("run", None, "run m"),
                node("t1", Some("run"), "turn 1"),
            ],
        });
        screen(&mut app, 50, 16);
        // Row 0 of the tree sits under the border and the blank row, at
        // y = 2.
        app.click_graph(5, 2);
        assert!(open_json(&app).contains("\"id\": \"run\""));
        app.handle_key(KeyEvent::from(KeyCode::Char('q')));
        assert_eq!(app.detail_view(), Some(&DetailView::Graph));
    }
}
