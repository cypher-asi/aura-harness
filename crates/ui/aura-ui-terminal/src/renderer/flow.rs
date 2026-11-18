//! The chat panel's GRAPH tab: the flow between an agent and the agents
//! it spawned, drawn as boxes in time order with one column per agent.
//!
//! ```text
//!             Lead                        coder #1
//!  ┌ ▾ ✓ prompt ────────────┐
//!  │ add a greeting         │
//!  ├ ✓ tool ────────────────┤
//!  │ task coder             │
//!  └────────────┬───────────┘  ┌ ▾ ✓ prompt ─── coder #1 ┐
//!               ├─────────────▶│ write greet()           │
//!               │              ├ ✓ reply ────────────────┤
//!               │              │ done                    │
//!  ┌ ✓ result ──────────────┐  └────────────┬────────────┘
//!  │ coder #1 replied       │◀──────────────┘
//!  └────────────────────────┘
//! ```
//!
//! Only the rows on screen are drawn, so a long flow costs no more per
//! frame than a short one.

use super::graph::status_glyph;
use super::panels::{body_block, render_scrollbar, truncate_record_info, ScrollView};
use super::text::display_width;
use crate::app::flow::{FlowBox, FlowEdge, FlowLayout, FlowState, FlowView, LaneSpan, BOX_ROWS};
use crate::events::{FlowKind, FlowNode};
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

const HINT: &str =
    "↑↓ move  ←→ lane  [ ] prompt  ⏎ open  Space fold  z all  f link  o agent  r record  Esc chat";

/// Draw the GRAPH tab in `area`, the chat panel below its header bar.
pub(super) fn render_flow(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let muted = Style::default().fg(theme.colors.muted);
    let mut block = body_block((is_focused, app.host_chrome()), theme);
    let hint = truncate_record_info(HINT, usize::from(area.width).saturating_sub(4));
    if is_focused && !hint.is_empty() {
        block = block.title_bottom(Line::from(Span::styled(format!(" {hint} "), muted)));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Nothing of the conversation is on screen to click or select.
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);

    app.flow_mut().ensure_layout();
    if inner.height < 3 || inner.width < 12 || app.flow().layout().boxes.is_empty() {
        app.flow_mut().set_view(None);
        let text = if app.flow().loaded() {
            "Nothing recorded yet. Prompts, model calls, tool calls and the agents \
             they start show up here as the agent works."
        } else {
            "Loading the graph…"
        };
        let padded = Rect {
            x: inner.x.saturating_add(1),
            width: inner.width.saturating_sub(2),
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, muted))).wrap(Wrap { trim: true }),
            padded,
        );
        return;
    }

    let names = Rect { height: 1, ..inner };
    let rows = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let (lane_width, visible_columns) = FlowView::lanes_in(rows.width, app.flow().layout().columns);
    let (first_row, first_column) = app
        .flow_mut()
        .sync_scroll(usize::from(rows.height), visible_columns);
    let view = FlowView {
        rows,
        first_row,
        first_column,
        lane_width,
    };
    app.flow_mut().set_view(Some(view));

    let flow = app.flow();
    let mut canvas = Canvas {
        buffer: frame.buffer_mut(),
        theme,
        flow,
        layout: flow.layout(),
        view,
    };
    canvas.lane_names(names, visible_columns);
    canvas.lifelines();
    canvas.arrows();
    canvas.boxes();

    let total = app.flow().layout().rows();
    render_scrollbar(
        frame,
        area,
        rows,
        ScrollView {
            total,
            visible: usize::from(rows.height),
            first: first_row,
        },
        is_focused,
        theme,
    );
}

/// How a box stands to the selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Selected,
    /// An arrow joins it to the selected box
    Linked,
    Other,
}

/// What is needed to draw the part of a flow that is on screen.
struct Canvas<'a> {
    buffer: &'a mut Buffer,
    theme: &'a Theme,
    flow: &'a FlowState,
    layout: &'a FlowLayout,
    view: FlowView,
}

impl Canvas<'_> {
    fn put(&mut self, x: u16, y: u16, symbol: &str, style: Style) {
        if let Some(cell) = self.buffer.cell_mut((x, y)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }

    fn muted(&self) -> Style {
        Style::default().fg(self.theme.colors.muted)
    }

    /// Flow rows on screen: the first, and one past the last.
    fn row_range(&self) -> (usize, usize) {
        let end = self.view.first_row + usize::from(self.view.rows.height);
        (self.view.first_row, end.min(self.layout.rows()))
    }

    /// Screen row of flow row `row`, which must be on screen.
    fn screen_y(&self, row: usize) -> u16 {
        let offset = u16::try_from(row.saturating_sub(self.view.first_row)).unwrap_or(0);
        self.view.rows.y + offset
    }

    fn standing(&self, index: usize, selected: Option<usize>) -> Standing {
        if selected == Some(index) {
            Standing::Selected
        } else if self.layout.edges.iter().any(|edge| {
            (Some(edge.from) == selected && edge.to == index)
                || (Some(edge.to) == selected && edge.from == index)
        }) {
            Standing::Linked
        } else {
            Standing::Other
        }
    }

    fn border(&self, standing: Standing) -> Style {
        match standing {
            Standing::Selected => Style::default()
                .fg(self.theme.colors.primary)
                .add_modifier(Modifier::BOLD),
            Standing::Linked => Style::default().fg(self.theme.colors.secondary),
            Standing::Other => self.muted(),
        }
    }

    /// The agent each column on screen belongs to, above the boxes. A
    /// column agents took turns in names the one at the top of the view.
    fn lane_names(&mut self, area: Rect, visible_columns: usize) {
        let (first_row, end_row) = self.row_range();
        for column in self.view.first_column..self.view.first_column + visible_columns {
            let Some((x, width)) = self.view.box_columns(column) else {
                break;
            };
            let Some(span) = self
                .layout
                .spans
                .iter()
                .filter(|span| span.column == column)
                .find(|span| span.last * BOX_ROWS + BOX_ROWS >= first_row)
            else {
                continue;
            };
            let name = truncate_record_info(
                &self.flow.lanes()[span.lane].name,
                usize::from(width).saturating_sub(2),
            );
            let style = if span.first * BOX_ROWS >= end_row {
                // Its first box is still below the view.
                self.muted()
            } else if span.lane == 0 {
                Style::default()
                    .fg(self.theme.colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.colors.secondary)
            };
            let name_width = u16::try_from(display_width(&name)).unwrap_or(width);
            let start = x + width.saturating_sub(name_width) / 2;
            self.buffer
                .set_stringn(start, area.y, &name, usize::from(width), style);
        }
        let muted = self.muted();
        if self.view.first_column > 0 {
            self.put(area.x, area.y, "◀", muted);
        }
        if self.view.first_column + visible_columns < self.layout.columns {
            self.put(area.x + area.width.saturating_sub(1), area.y, "▶", muted);
        }
    }

    /// First flow row of a lane's line: the top of its first box, or the
    /// label row of an earlier box one of its arrows goes back to.
    fn line_start(&self, span: &LaneSpan) -> usize {
        let own = self.layout.boxes[span.first].lane == span.lane;
        span.first * BOX_ROWS + usize::from(!own)
    }

    /// Each lane's line, from its first box to its last or to the last
    /// box one of its arrows goes to. The boxes are drawn over it.
    fn lifelines(&mut self) {
        let (first_row, end_row) = self.row_range();
        let style = self.muted();
        for span in &self.layout.spans {
            let Some(x) = self.view.lifeline(span.column) else {
                continue;
            };
            let from = self.line_start(span).max(first_row);
            let to = (span.last * BOX_ROWS + BOX_ROWS).min(end_row);
            for row in from..to {
                let y = self.screen_y(row);
                self.put(x, y, "│", style);
            }
        }
    }

    /// The arrows into the boxes on screen, each on its box's label row;
    /// those of the selected box last, so they are drawn whole.
    fn arrows(&mut self) {
        let (first_row, end_row) = self.row_range();
        let selected = self.flow.selected_box();
        let mut arrows: Vec<(bool, FlowEdge)> = Vec::new();
        for index in first_row / BOX_ROWS..self.layout.boxes.len() {
            let row = index * BOX_ROWS + 1;
            if row >= end_row {
                break;
            }
            if row < first_row {
                continue;
            }
            arrows.extend(self.layout.edges_into(index).iter().map(|edge| {
                let hot = selected == Some(edge.from) || selected == Some(edge.to);
                (hot, *edge)
            }));
        }
        arrows.sort_by_key(|(hot, _)| *hot);
        for (hot, edge) in arrows {
            let style = if hot {
                Style::default()
                    .fg(self.theme.colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                self.muted()
            };
            self.arrow(edge, style);
        }
    }

    /// One arrow: from the source lane's line, along the label row of the
    /// box it points at, to that box's side.
    fn arrow(&mut self, edge: FlowEdge, style: Style) {
        let source = self.layout.boxes[edge.from];
        let target = self.layout.boxes[edge.to];
        let Some((box_x, box_width)) = self.view.box_columns(target.column) else {
            return;
        };
        if source.column == target.column {
            return;
        }
        let y = self.screen_y(edge.to * BOX_ROWS + 1);
        let rightward = source.column < target.column;
        let junction = self.view.lifeline(source.column);
        let area = self.view.rows;
        // Cells of the line, between the junction (or the edge of the
        // view, when the source lane is off screen) and the arrowhead.
        let (line, head, head_symbol) = if rightward {
            let start = junction.map_or(area.x, |x| x + 1);
            (start..box_x.saturating_sub(1), box_x.saturating_sub(1), "▶")
        } else {
            let head = box_x + box_width;
            let end = junction.unwrap_or(area.x + area.width);
            (head + 1..end, head, "◀")
        };
        for x in line {
            let crossing = self
                .buffer
                .cell((x, y))
                .is_some_and(|cell| cell.symbol() == "│");
            self.put(x, y, if crossing { "┼" } else { "─" }, style);
        }
        self.put(head, y, head_symbol, style);

        let (Some(x), Some(span)) = (junction, self.layout.span_of(edge.from)) else {
            return;
        };
        let above = self.line_start(span) < edge.to * BOX_ROWS + 1;
        let below = edge.to < span.last;
        let symbol = match (above, below, rightward) {
            (true, true, true) => "├",
            (true, true, false) => "┤",
            (true, false, true) => "└",
            (true, false, false) => "┘",
            (false, true, true) => "┌",
            (false, true, false) => "┐",
            (false, false, _) => "─",
        };
        self.put(x, y, symbol, style);
    }

    /// The boxes on screen. A box's bottom border is on the next box's
    /// title row: joined to that title when the next box is in the same
    /// lane, and its own otherwise.
    fn boxes(&mut self) {
        let (first_row, end_row) = self.row_range();
        let selected = self.flow.selected_box();
        let count = self.layout.boxes.len();
        for row in first_row..end_row {
            let index = row / BOX_ROWS;
            let y = self.screen_y(row);
            let joined = index > 0
                && index < count
                && self.layout.boxes[index - 1].lane == self.layout.boxes[index].lane;
            if row % BOX_ROWS != 0 {
                if index < count {
                    self.label_row(index, y, self.standing(index, selected));
                }
                continue;
            }
            if index > 0 && !joined {
                self.bottom_border(index - 1, y, self.standing(index - 1, selected));
            }
            if index < count {
                let mut standing = self.standing(index, selected);
                if joined && selected == Some(index - 1) {
                    // The border belongs to the selected box above too.
                    standing = Standing::Selected;
                }
                self.title_row(index, y, joined, standing);
            }
        }
    }

    fn bottom_border(&mut self, index: usize, y: u16, standing: Standing) {
        let placed = self.layout.boxes[index];
        let Some((x, width)) = self.view.box_columns(placed.column) else {
            return;
        };
        let style = self.border(standing);
        self.put(x, y, "└", style);
        for offset in 1..width - 1 {
            self.put(x + offset, y, "─", style);
        }
        self.put(x + width - 1, y, "┘", style);
        let continues = self
            .layout
            .span_of(index)
            .is_some_and(|span| span.last > index);
        if continues {
            // The lane's line goes on below the box.
            self.put(x + width / 2, y, "┬", style);
        }
    }

    fn title_row(&mut self, index: usize, y: u16, joined: bool, standing: Standing) {
        let placed = self.layout.boxes[index];
        let Some((x, width)) = self.view.box_columns(placed.column) else {
            return;
        };
        let node = &self.flow.nodes()[placed.node];
        let border = self.border(standing);
        let (left, right) = if joined {
            ("├", "┤")
        } else {
            ("┌", "┐")
        };
        self.put(x, y, left, border);
        for offset in 1..width - 1 {
            self.put(x + offset, y, "─", border);
        }
        self.put(x + width - 1, y, right, border);

        // ` ▾ ✓ prompt `: fold marker (turns only), status, kind.
        let (glyph, glyph_color) = status_glyph(node.status, self.theme);
        let mut parts: Vec<(&str, Style)> = Vec::new();
        if placed.steps > 0 {
            let marker = if placed.folded { "▸" } else { "▾" };
            parts.push((marker, Style::default().fg(self.theme.colors.secondary)));
        }
        parts.push((glyph, Style::default().fg(glyph_color)));
        parts.push((
            node.kind.label(),
            Style::default()
                .fg(kind_color(node.kind, self.theme))
                .add_modifier(Modifier::BOLD),
        ));
        let inner_end = x + width - 1;
        let mut at = x + 1;
        self.put(at, y, " ", border);
        at += 1;
        for (text, style) in parts {
            let text_width = u16::try_from(display_width(text)).unwrap_or(0);
            if at + text_width + 1 >= inner_end {
                break;
            }
            self.buffer
                .set_stringn(at, y, text, usize::from(text_width), style);
            self.put(at + text_width, y, " ", border);
            at += text_width + 1;
        }

        // ` 1.2k/300 tok ` against the right corner, one `─` after the kind.
        let room = usize::from(inner_end.saturating_sub(at + 1));
        let facts = truncate_record_info(&self.facts(node, placed), room.saturating_sub(2));
        if room >= 5 && !facts.is_empty() {
            let facts_width = u16::try_from(display_width(&facts)).unwrap_or(0);
            let start = inner_end - facts_width - 2;
            let style = self.muted();
            self.put(start, y, " ", style);
            self.buffer
                .set_stringn(start + 1, y, &facts, usize::from(facts_width), style);
            self.put(start + 1 + facts_width, y, " ", style);
        }
    }

    /// What a box's title says beside its kind: the agent for a prompt in
    /// a spawned agent's lane, how many steps a folded turn hides, and the
    /// node's own detail.
    fn facts(&self, node: &FlowNode, placed: FlowBox) -> String {
        let mut facts: Vec<String> = Vec::new();
        if node.kind == FlowKind::Prompt && placed.lane != 0 {
            facts.push(self.flow.lanes()[placed.lane].name.clone());
        }
        if placed.folded {
            let unit = if placed.steps == 1 { "step" } else { "steps" };
            facts.push(format!("+{} {unit}", placed.steps));
        }
        if !node.detail.is_empty() {
            facts.push(node.detail.clone());
        }
        facts.join(" · ")
    }

    fn label_row(&mut self, index: usize, y: u16, standing: Standing) {
        let placed = self.layout.boxes[index];
        let Some((x, width)) = self.view.box_columns(placed.column) else {
            return;
        };
        let node = &self.flow.nodes()[placed.node];
        let border = self.border(standing);
        let text = if standing == Standing::Selected {
            Style::default()
                .bg(self.theme.colors.selection)
                .fg(self.theme.colors.on_selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(match node.kind {
                FlowKind::Model | FlowKind::Event => self.theme.colors.muted,
                _ => self.theme.colors.foreground,
            })
        };
        self.put(x, y, "│", border);
        for offset in 1..width - 1 {
            self.put(x + offset, y, " ", text);
        }
        self.put(x + width - 1, y, "│", border);
        let room = usize::from(width.saturating_sub(4));
        let label = truncate_record_info(&node.label, room);
        self.buffer.set_stringn(x + 2, y, &label, room, text);
    }
}

fn kind_color(kind: FlowKind, theme: &Theme) -> Color {
    match kind {
        FlowKind::Prompt => theme.colors.primary,
        FlowKind::Spawn | FlowKind::Message | FlowKind::Result => theme.colors.secondary,
        FlowKind::Model | FlowKind::Event => theme.colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => theme.colors.foreground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatTab;
    use crate::events::{FlowLane, TraceNodeStatus, UiCommand};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, Terminal};

    const LEAD: &str = "aa";
    const CODER: &str = "bb";

    fn node(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
        FlowNode {
            id: format!("{agent}:{seq}"),
            agent_id: agent.into(),
            kind,
            label: label.into(),
            status: TraceNodeStatus::Ok,
            seq,
            at_ms,
            ..FlowNode::default()
        }
    }

    /// A lead that gives a coder a task and gets its reply.
    fn app_with_flow() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        let mut call = node(LEAD, 2, 110, FlowKind::Tool, "task coder");
        call.links = vec![("spawn".into(), "bb:1".into())];
        call.detail = "4.2s".into();
        let mut reply = node(CODER, 2, 130, FlowKind::Reply, "done");
        reply.links = vec![("return".into(), "aa:4".into())];
        let mut pending = node(LEAD, 5, 150, FlowKind::Model, "sonnet");
        pending.status = TraceNodeStatus::Pending;
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: vec![
                FlowLane {
                    agent_id: LEAD.into(),
                    name: "Lead".into(),
                    parent: None,
                },
                FlowLane {
                    agent_id: CODER.into(),
                    name: "coder #1".into(),
                    parent: Some(LEAD.into()),
                },
            ],
            nodes: vec![
                node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
                call,
                node(CODER, 1, 120, FlowKind::Prompt, "write greet()"),
                reply,
                node(LEAD, 4, 140, FlowKind::Result, "coder #1 replied"),
                pending,
            ],
        });
        app
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::cyber();
        terminal
            .draw(|frame| render_flow(frame, frame.area(), app, &theme, true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    #[test]
    fn draws_each_agent_in_its_lane_with_arrows_between_them() {
        let mut app = app_with_flow();
        let rows = screen(&mut app, 62, 16);
        let expected = [
            "┌────────────────────────────────────────────────────────────┐",
            "│             Lead                        coder #1           │",
            "│ ┌ ▾ ✓ prompt ──────────────┐                               │",
            "│ │ add a greeting           │                               │",
            "│ ├ ✓ tool ──────────── 4.2s ┤                               │",
            "│ │ task coder               │                               │",
            "│ └─────────────┬────────────┘  ┌ ▾ ✓ prompt ──── coder #1 ┐ │",
            "│               ├──────────────▶│ write greet()            │ │",
            "│               │               ├ ✓ reply ─────────────────┤ │",
            "│               │               │ done                     │ │",
            "│ ┌ ✓ result ────────────────┐  └─────────────┬────────────┘ │",
            "│ │ coder #1 replied         │◀───────────────┘              │",
            "│ ├ ● model ─────────────────┤                               │",
            "│ │ sonnet                   │                               │",
            "│ └──────────────────────────┘                               │",
        ];
        assert_eq!(rows[..expected.len()], expected, "\n{}", rows.join("\n"));
    }

    #[test]
    fn a_folded_turn_says_how_much_it_hides() {
        let mut app = app_with_flow();
        screen(&mut app, 62, 16);
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char(' '));
        let rows = screen(&mut app, 62, 16).join("\n");
        assert!(rows.contains("┌ ▸ ✓ prompt ──── +3 steps ┐"), "{rows}");
        assert!(!rows.contains("task coder"), "{rows}");
        // The arrows now leave from and come back to the prompt.
        assert!(rows.contains("└──────────────▶│ write greet()"), "{rows}");
        assert!(
            rows.contains("│ add a greeting           │◀───────────────┐"),
            "{rows}"
        );
    }

    #[test]
    fn only_the_rows_on_screen_are_drawn_and_the_selection_stays_in_view() {
        let mut app = app_with_flow();
        // Three rows of boxes: the newest box, selected, fills them.
        let rows = screen(&mut app, 62, 6).join("\n");
        assert!(rows.contains("│ sonnet"), "{rows}");
        assert!(!rows.contains("add a greeting"), "{rows}");

        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 62, 6).join("\n");
        assert!(rows.contains("add a greeting"), "{rows}");
        assert!(!rows.contains("sonnet"), "{rows}");
    }

    #[test]
    fn lanes_that_do_not_fit_scroll_sideways_with_the_selection() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 34, 16).join("\n");
        assert!(rows.contains("Lead"), "{rows}");
        assert!(!rows.contains("write greet()"), "{rows}");
        assert!(rows.contains('▶'), "more lanes to the right: {rows}");

        press(&mut app, KeyCode::Right);
        let rows = screen(&mut app, 34, 16).join("\n");
        assert!(rows.contains("write greet()"), "{rows}");
        assert!(!rows.contains("add a greeting"), "{rows}");
    }

    #[test]
    fn clicking_selects_a_box_then_opens_it_and_the_marker_folds() {
        let click = |app: &mut App, column: u16, row: u16| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::empty(),
            });
        };
        let mut app = app_with_flow();
        let theme = Theme::cyber();
        let draw = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
            terminal
                .draw(|frame| crate::renderer::render(frame, app, &theme))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let find = |text: &str| {
                (0..30u16).find_map(|y| {
                    let row: String = (0..160u16).map(|x| buffer[(x, y)].symbol()).collect();
                    row.find(text)
                        .map(|at| (u16::try_from(row[..at].chars().count()).unwrap(), y))
                })
            };
            (find("write greet()"), find("▾ ✓ prompt"))
        };

        let (label, marker) = draw(&mut app);
        let (x, y) = label.unwrap();
        click(&mut app, x, y);
        assert_eq!(app.flow().selected_node().unwrap().id, "bb:1");
        assert!(app.detail_view().is_none());
        click(&mut app, x, y);
        assert!(app.detail_view().is_some(), "the second click opens it");
        press(&mut app, KeyCode::Esc);

        // The lead's turn is the first with a marker.
        let (x, y) = marker.unwrap();
        click(&mut app, x, y);
        app.flow_mut().ensure_layout();
        assert!(app.flow().layout().boxes[0].folded);

        // The wheel moves the view and leaves the selection alone.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: x,
            row: y,
            modifiers: KeyModifiers::empty(),
        });
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");
    }

    #[test]
    fn says_so_while_loading_and_when_nothing_is_recorded() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        let rows = screen(&mut app, 60, 8).join("\n");
        assert!(rows.contains("Loading the graph"), "{rows}");

        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: Vec::new(),
            nodes: Vec::new(),
        });
        let rows = screen(&mut app, 60, 8).join("\n");
        assert!(rows.contains("Nothing recorded yet"), "{rows}");
    }
}
