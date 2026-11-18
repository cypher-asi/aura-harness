//! The chat panel's GRAPH tab: what an agent and the agents it spawned
//! did, as cards wired together on a canvas, drawn with box characters.
//!
//! ```text
//!   Lead
//!   ╭ ✓ prompt ──────────────────────╮      ╭ ✓ model ────── 1.2k/300 · 4s ╮
//!   │ add a greeting                 ●─────▶│ sonnet                       ●─▶
//!   ╰────────────────────────────────╯      │ ✓ task coder · write greet() ●─╮
//!                                           ╰──────────────────────────────╯ │
//!   coder #1                                                                 │
//!                                                                            ╰▶╭ ✓ prompt ─
//! ```
//!
//! Only the part of the canvas on screen is drawn, so a long flow costs no
//! more per frame than a short one. A front end that draws the canvas
//! itself ([`crate::App::set_host_canvas`]) gets the cells left blank.

use super::graph::status_glyph;
use super::panels::{body_block, render_scrollbar, truncate_record_info, ScrollView};
use super::text::display_width;
use crate::app::flow::layout::{FlowLayout, Wire, WireKind, CARD_WIDTH};
use crate::app::flow::{FlowState, FlowView};
use crate::events::FlowKind;
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthChar;

const HINT: &str = "←→ card  ↑↓ line  [ ] prompt  ⏎ open  Space fold  z all  f link  o agent  r record  drag pan  Esc chat";

/// Canvas columns and rows between the dots of the grid behind the cards.
const DOT_COLUMNS: usize = 4;
const DOT_ROWS: usize = 2;

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
    if inner.height < 3 || inner.width < 12 || app.flow().layout().cards.is_empty() {
        app.flow_mut().set_view(None, false);
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

    let pan = app
        .flow_mut()
        .sync_view(f32::from(inner.width), f32::from(inner.height));
    let hosted = app.host_canvas();
    app.flow_mut()
        .set_view(Some(FlowView { area: inner, pan }), hosted);
    let first = (whole(pan.0), whole(pan.1));
    if hosted {
        let buffer = frame.buffer_mut();
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_skip(true);
                }
            }
        }
    } else {
        let flow = app.flow();
        Canvas {
            buffer: frame.buffer_mut(),
            theme,
            flow,
            layout: flow.layout(),
            area: inner,
            first,
        }
        .draw();
    }

    let total = app.flow().layout().height;
    render_scrollbar(
        frame,
        area,
        inner,
        ScrollView {
            total,
            visible: usize::from(inner.height),
            first: first.1,
        },
        is_focused,
        theme,
    );
}

/// The whole cells in a canvas position.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole(at: f32) -> usize {
    at.max(0.0).floor() as usize
}

/// How a card stands to the selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Selected,
    /// A wire joins it to the selected card
    Linked,
    Other,
}

/// What is needed to draw the part of the canvas that is on screen.
struct Canvas<'a> {
    buffer: &'a mut Buffer,
    theme: &'a Theme,
    flow: &'a FlowState,
    layout: &'a FlowLayout,
    area: Rect,
    /// Canvas cell at the top left of `area`
    first: (usize, usize),
}

impl Canvas<'_> {
    /// The screen cell of canvas cell `(x, y)`, when it is on screen.
    fn screen(&self, x: usize, y: usize) -> Option<(u16, u16)> {
        let dx = u16::try_from(x.checked_sub(self.first.0)?).ok()?;
        let dy = u16::try_from(y.checked_sub(self.first.1)?).ok()?;
        (dx < self.area.width && dy < self.area.height)
            .then(|| (self.area.x + dx, self.area.y + dy))
    }

    fn put(&mut self, x: usize, y: usize, symbol: &str, style: Style) {
        if let Some(at) = self.screen(x, y) {
            if let Some(cell) = self.buffer.cell_mut(at) {
                cell.set_symbol(symbol).set_style(style);
            }
        }
    }

    fn symbol(&self, x: usize, y: usize) -> &str {
        self.screen(x, y)
            .and_then(|at| self.buffer.cell(at))
            .map_or("", |cell| cell.symbol())
    }

    /// `text` from canvas cell `(x, y)`, in at most `width` columns.
    fn text(&mut self, x: usize, y: usize, text: &str, width: usize, style: Style) {
        let mut at = 0;
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if at + w > width {
                break;
            }
            self.put(x + at, y, c.encode_utf8(&mut buf), style);
            if w == 2 {
                self.put(x + at + 1, y, " ", style);
            }
            at += w;
        }
    }

    fn muted(&self) -> Style {
        Style::default().fg(self.theme.colors.muted)
    }

    fn draw(&mut self) {
        self.dots();
        self.names();
        let selected = self.flow.selected_card();
        let mut wires: Vec<(bool, Wire)> = self
            .layout
            .wires
            .iter()
            .map(|wire| {
                (
                    Some(wire.from) == selected || Some(wire.to) == selected,
                    *wire,
                )
            })
            .collect();
        wires.sort_by_key(|(hot, _)| *hot);
        for (hot, wire) in &wires {
            self.wire(*wire, self.wire_style(wire.kind, *hot));
        }
        let linked = self.flow.linked_cards();
        for card in 0..self.layout.cards.len() {
            let standing = if selected == Some(card) {
                Standing::Selected
            } else if linked.contains(&card) {
                Standing::Linked
            } else {
                Standing::Other
            };
            self.card(card, standing);
        }
        for (hot, wire) in &wires {
            let (x, y) = self.layout.port_out(wire.from, wire.row);
            self.put(x, y, "●", self.wire_style(wire.kind, *hot));
        }
    }

    /// The grid of dots the cards sit on.
    fn dots(&mut self) {
        let style = self.muted().add_modifier(Modifier::DIM);
        for dy in 0..usize::from(self.area.height) {
            let y = self.first.1 + dy;
            if y % DOT_ROWS != 1 {
                continue;
            }
            for dx in 0..usize::from(self.area.width) {
                let x = self.first.0 + dx;
                if x % DOT_COLUMNS == 0 {
                    self.put(x, y, "·", style);
                }
            }
        }
    }

    /// Each agent's name above its cards, kept on screen while any of
    /// them is.
    fn names(&mut self) {
        for span in &self.layout.spans {
            let y = self.layout.bands[span.band].y;
            let start = FlowLayout::column_x(span.first);
            let end = FlowLayout::column_x(span.last) + CARD_WIDTH;
            let name = truncate_record_info(&self.flow.lanes()[span.lane].name, end - start);
            let width = display_width(&name);
            let x = (self.first.0 + 1).clamp(start, end - width);
            let style = if span.lane == 0 {
                Style::default()
                    .fg(self.theme.colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(self.theme.colors.secondary)
                    .add_modifier(Modifier::BOLD)
            };
            self.text(x, y, &name, width, style);
        }
    }

    fn wire_style(&self, kind: WireKind, hot: bool) -> Style {
        if hot {
            return Style::default()
                .fg(self.theme.colors.primary)
                .add_modifier(Modifier::BOLD);
        }
        Style::default().fg(match kind {
            WireKind::Next => self.theme.colors.muted,
            WireKind::Spawn | WireKind::Link => self.theme.colors.secondary,
            WireKind::Return => self.theme.colors.success,
            WireKind::Message => self.theme.colors.warning,
        })
    }

    /// One wire: out of its port, along to the gutter before the card it
    /// goes to, up or down there, and into that card's side.
    fn wire(&mut self, wire: Wire, style: Style) {
        let (from_x, from_y) = self.layout.port_out(wire.from, wire.row);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        if to_x < from_x + 3 {
            return;
        }
        let head = to_x - 1;
        if from_y == to_y {
            self.horizontal(from_x + 1..head, from_y, style);
        } else {
            let turn = (to_x - 2 - wire.track).max(from_x + 1);
            self.horizontal(from_x + 1..turn, from_y, style);
            let (top, bottom) = (from_y.min(to_y), from_y.max(to_y));
            for y in top + 1..bottom {
                self.join(turn, y, UP | DOWN, style);
            }
            let (leave, arrive) = if to_y > from_y {
                (LEFT | DOWN, UP | RIGHT)
            } else {
                (LEFT | UP, DOWN | RIGHT)
            };
            self.join(turn, from_y, leave, style);
            self.join(turn, to_y, arrive, style);
            self.horizontal(turn + 1..head, to_y, style);
        }
        self.put(head, to_y, "▶", style);
    }

    fn horizontal(&mut self, xs: std::ops::Range<usize>, y: usize, style: Style) {
        for x in xs {
            self.join(x, y, LEFT | RIGHT, style);
        }
    }

    /// A piece of wire with `ends` at `(x, y)`, joined to any wire already
    /// there: `─` over `│` makes `┼`, over `╭` makes `┬`.
    fn join(&mut self, x: usize, y: usize, ends: u8, style: Style) {
        let ends = ends | ends_of(self.symbol(x, y));
        self.put(x, y, glyph_of(ends), style);
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

    fn selection(&self) -> Style {
        Style::default()
            .bg(self.theme.colors.selection)
            .fg(self.theme.colors.on_selection)
            .add_modifier(Modifier::BOLD)
    }

    fn card(&mut self, card: usize, standing: Standing) {
        let rect = self.layout.card_rect(card);
        let on_screen = rect.right() > self.first.0
            && rect.x < self.first.0 + usize::from(self.area.width)
            && rect.bottom() > self.first.1
            && rect.y < self.first.1 + usize::from(self.area.height);
        if !on_screen {
            return;
        }
        let border = self.border(standing);
        let (x, y, w) = (rect.x, rect.y, rect.width);
        let bottom = rect.bottom() - 1;
        for line in y..=bottom {
            let (left, fill, right) = if line == y {
                ("╭", "─", "╮")
            } else if line == bottom {
                ("╰", "─", "╯")
            } else {
                ("│", " ", "│")
            };
            self.put(x, line, left, border);
            for column in x + 1..x + w - 1 {
                self.put(
                    column,
                    line,
                    fill,
                    if fill == " " {
                        Style::default()
                    } else {
                        border
                    },
                );
            }
            self.put(x + w - 1, line, right, border);
        }
        self.title(card, border);

        let placed = &self.layout.cards[card];
        let selected = self.flow.selected_place().filter(|(at, _)| *at == card);
        let node = &self.flow.nodes()[placed.head];
        let head = if selected == Some((card, None)) {
            self.selection()
        } else {
            let style = Style::default().fg(self.theme.colors.foreground);
            match node.kind {
                FlowKind::Prompt | FlowKind::Reply => style.add_modifier(Modifier::BOLD),
                _ => style,
            }
        };
        self.line_fill(card, 1, head);
        self.text(x + 2, y + 1, &node.label, w - 4, head);

        let shown = placed.shown_rows();
        for row in 0..placed.lines() {
            let line = 2 + row;
            if row == shown {
                // The last line of a card with more rows than lines.
                match self.flow.last_line_row(card) {
                    Some(hidden) => self.row(card, hidden, line, true),
                    None => {
                        let more = self.flow.more_label(card);
                        self.text(x + 2, y + line, &more, w - 4, self.muted());
                    }
                }
                continue;
            }
            let is_selected = selected == Some((card, Some(row)));
            self.row(card, row, line, is_selected);
        }
    }

    /// `╭ ▾ ✓ model ─────── 1.2k/300 · 4s ╮`: fold marker (turns only),
    /// status, kind, and the card's facts against the right corner.
    fn title(&mut self, card: usize, border: Style) {
        let rect = self.layout.card_rect(card);
        let placed = &self.layout.cards[card];
        let node = &self.flow.nodes()[placed.head];
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
        let end = rect.right() - 1;
        let mut at = rect.x + 1;
        self.put(at, rect.y, " ", border);
        at += 1;
        for (text, style) in parts {
            let width = display_width(text);
            if at + width + 1 >= end {
                break;
            }
            self.text(at, rect.y, text, width, style);
            self.put(at + width, rect.y, " ", border);
            at += width + 1;
        }
        let room = end.saturating_sub(at + 1);
        let facts = truncate_record_info(&self.flow.card_facts(card), room.saturating_sub(2));
        if room >= 5 && !facts.is_empty() {
            let width = display_width(&facts);
            let start = end - width - 2;
            self.put(start, rect.y, " ", self.muted());
            self.text(start + 1, rect.y, &facts, width, self.muted());
            self.put(start + 1 + width, rect.y, " ", self.muted());
        }
    }

    /// Paint line `line` of card `card`, inside its borders, in `style`.
    fn line_fill(&mut self, card: usize, line: usize, style: Style) {
        let rect = self.layout.card_rect(card);
        for x in rect.x + 1..rect.right() - 1 {
            self.put(x, rect.y + line, " ", style);
        }
    }

    /// `✓ read_file src/lib.rs          12ms`: a row's status, label, and
    /// its detail against the right border.
    fn row(&mut self, card: usize, row: usize, line: usize, selected: bool) {
        let rect = self.layout.card_rect(card);
        let node = &self.flow.nodes()[self.layout.cards[card].rows[row]];
        let (glyph, glyph_color) = status_glyph(node.status, self.theme);
        let base = if selected {
            self.selection()
        } else {
            Style::default().fg(match node.kind {
                FlowKind::Event => self.theme.colors.muted,
                FlowKind::Spawn => self.theme.colors.secondary,
                _ => self.theme.colors.foreground,
            })
        };
        if selected {
            self.line_fill(card, line, base);
        }
        let y = rect.y + line;
        let room = rect.width - 4;
        let glyph_style = if selected { base } else { base.fg(glyph_color) };
        self.text(rect.x + 2, y, glyph, 1, glyph_style);
        let detail = truncate_record_info(&node.detail, room / 3);
        let detail_width = display_width(&detail);
        let label_room = room - 2 - detail_width - usize::from(detail_width > 0);
        let label = truncate_record_info(&node.label, label_room);
        self.text(rect.x + 4, y, &label, label_room, base);
        if detail_width > 0 {
            let detail_style = if selected { base } else { self.muted() };
            self.text(
                rect.right() - 2 - detail_width,
                y,
                &detail,
                detail_width,
                detail_style,
            );
        }
    }
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Box glyphs by the ends they join.
const GLYPHS: [(&str, u8); 11] = [
    ("─", LEFT | RIGHT),
    ("│", UP | DOWN),
    ("╭", DOWN | RIGHT),
    ("╮", DOWN | LEFT),
    ("╰", UP | RIGHT),
    ("╯", UP | LEFT),
    ("├", UP | DOWN | RIGHT),
    ("┤", UP | DOWN | LEFT),
    ("┬", DOWN | LEFT | RIGHT),
    ("┴", UP | LEFT | RIGHT),
    ("┼", UP | DOWN | LEFT | RIGHT),
];

fn ends_of(symbol: &str) -> u8 {
    GLYPHS
        .iter()
        .find(|(glyph, _)| *glyph == symbol)
        .map_or(0, |(_, ends)| *ends)
}

fn glyph_of(ends: u8) -> &'static str {
    GLYPHS
        .iter()
        .find(|(_, glyph_ends)| *glyph_ends == ends)
        .map_or(
            if ends & (UP | DOWN) == 0 {
                "─"
            } else {
                "│"
            },
            |(glyph, _)| glyph,
        )
}

fn kind_color(kind: FlowKind, theme: &Theme) -> Color {
    match kind {
        FlowKind::Prompt => theme.colors.primary,
        FlowKind::Model | FlowKind::Spawn | FlowKind::Message | FlowKind::Result => {
            theme.colors.secondary
        }
        FlowKind::Event => theme.colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => theme.colors.foreground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatTab;
    use crate::events::{FlowLane, FlowNode, TraceNodeStatus, UiCommand};
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

    /// A lead whose model call gives a coder a task, and gets its reply.
    fn app_with_flow() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        let mut model = node(LEAD, 2, 105, FlowKind::Model, "sonnet");
        model.detail = "1.2k/300 · 4s".into();
        let mut read = node(LEAD, 3, 108, FlowKind::Tool, "read_file src/lib.rs");
        read.detail = "12ms".into();
        let mut call = node(LEAD, 4, 110, FlowKind::Tool, "task coder · write greet()");
        call.links = vec![("spawn".into(), "bb:1".into())];
        call.detail = "4.2s".into();
        let mut reply = node(CODER, 2, 130, FlowKind::Reply, "done");
        reply.links = vec![("return".into(), "aa:5".into())];
        let mut pending = node(LEAD, 6, 150, FlowKind::Model, "sonnet");
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
                model,
                read,
                call,
                node(CODER, 1, 120, FlowKind::Prompt, "write greet()"),
                reply,
                node(LEAD, 5, 140, FlowKind::Result, "coder #1 replied"),
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

    fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }

    /// The whole screen, 160 × 30, as the backend got it.
    fn whole_screen(app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// Where `text` first is on screen.
    fn find(buffer: &Buffer, text: &str) -> Option<(u16, u16)> {
        (0..buffer.area.height).find_map(|y| {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.find(text)
                .map(|at| (u16::try_from(row[..at].chars().count()).unwrap(), y))
        })
    }

    #[test]
    fn cards_sit_on_a_dot_grid_wired_port_to_port() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 84, 12);
        let expected = [
            "│  Lead                                                                            ┃",
            "│· ╭ ▾ ✓ prompt ────────────────────╮·   · ╭ ✓ model ──────── 1.2k/300 · 4s ╮·   · ┃",
            "│  │ add a greeting                 ●─────▶│ sonnet                         ●──────┃",
            "│· ╰────────────────────────────────╯·   · │ ✓ read_file src/lib.rs    12ms │·   · ┃",
            "│                                          │ ✓ task coder · write gre… 4.2s ●────╮ ┃",
            "│·   ·   ·   ·   ·   ·   ·   ·   ·   ·   · ╰────────────────────────────────╯·   │ ┃",
            "│                                                                                │ ┃",
            "│·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   │ ┃",
            "│                                                                                │ │",
            "│·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ·   ╰▶│",
        ];
        assert_eq!(rows[1..=10], expected, "\n{}", rows.join("\n"));
    }

    #[test]
    fn wires_into_one_port_join() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 200, 16).join("\n");
        assert!(rows.contains("─┬▶│ coder #1 replied"), "{rows}");
        // Drawn on top, the selection's wires still join the others.
        press(&mut app, KeyCode::Right);
        let rows = screen(&mut app, 200, 16).join("\n");
        assert!(rows.contains("─┬▶│ coder #1 replied"), "{rows}");
    }

    #[test]
    fn a_folded_turn_is_its_prompt_and_says_how_much_it_hides() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char(' '));
        let rows = screen(&mut app, 120, 16).join("\n");
        assert!(
            rows.contains("╭ ▸ ✓ prompt ────────── +3 steps ╮"),
            "{rows}"
        );
        assert!(!rows.contains("sonnet"), "{rows}");
        assert!(
            rows.contains("╰▶│ write greet()"),
            "the spawn comes from the prompt\n{rows}"
        );
    }

    #[test]
    fn the_view_follows_the_selection_across_the_canvas() {
        let mut app = app_with_flow();
        let rows = screen(&mut app, 80, 16).join("\n");
        assert!(rows.contains("● model"), "the newest card, pending\n{rows}");
        assert!(!rows.contains("add a greeting"), "{rows}");
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 80, 16).join("\n");
        assert!(rows.contains("add a greeting"), "{rows}");

        // Panned, the agent's name stays at the left while its cards are on
        // screen.
        app.pan_flow(50.0, 0.0);
        let rows = screen(&mut app, 80, 16);
        assert!(rows[1].starts_with("│ Lead"), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("add a greeting"));
    }

    #[test]
    fn clicks_select_and_open_lines_and_drags_and_the_wheel_pan() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        let label = find(&buffer, "sonnet").unwrap();
        let marker = find(&buffer, "▾ ✓ prompt").unwrap();
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse(&mut app, down, label);
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");
        assert!(app.detail_view().is_none());
        mouse(&mut app, down, label);
        assert!(app.detail_view().is_some(), "the second click opens it");
        press(&mut app, KeyCode::Esc);

        // The lead's turn is the first with a marker.
        mouse(&mut app, down, marker);
        app.flow_mut().ensure_layout();
        assert!(app.flow().layout().cards[0].folded);
        press(&mut app, KeyCode::Char(' '));

        // A drag on empty canvas moves the view with the pointer.
        let buffer = whole_screen(&mut app);
        let (x, y) = find(&buffer, "add a greeting").unwrap();
        let empty = (x, y + 3);
        let pan = app.flow().pan();
        mouse(&mut app, down, empty);
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            (x - 5, y + 3),
        );
        mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            (x - 5, y + 3),
        );
        assert_eq!(app.flow().pan(), (pan.0 + 5.0, pan.1));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");

        // The wheel moves the view and leaves the selection alone.
        let _ = whole_screen(&mut app);
        let pan = app.flow().pan();
        mouse(&mut app, MouseEventKind::ScrollLeft, empty);
        assert!(app.flow().pan().0 < pan.0);
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");
    }

    #[test]
    fn a_front_end_that_draws_the_canvas_gets_it_blank() {
        let mut app = app_with_flow();
        app.set_host_canvas(true);
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        assert!(find(&buffer, "add a greeting").is_none());
        let canvas = app.flow_canvas().expect("the canvas is on screen");
        assert_eq!(app.flow_canvas_clip(), [canvas]);
        for y in canvas.top()..canvas.bottom() {
            for x in canvas.left()..canvas.right() {
                let cell = &buffer[(x, y)];
                assert!(!cell.skip && cell.symbol() == " ", "({x}, {y})");
            }
        }
        assert_eq!(
            buffer[(canvas.x - 1, canvas.y)].symbol(),
            "│",
            "the border is drawn"
        );

        // Its clicks come back in canvas cells; terminal clicks are not its.
        let (x, y) = (canvas.x + 5, canvas.y + 2);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), (x, y));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");
        assert!(app.click_flow_at(46.5, 2.5));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");

        // Away from the tab, there is no canvas to draw.
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        let _ = whole_screen(&mut app);
        assert!(app.flow_canvas().is_none());
        assert!(app.flow_canvas_clip().is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));

        // A menu over the canvas is left out of where it is drawn.
        app.handle_key(KeyEvent::from(KeyCode::Char('/')));
        let buffer = whole_screen(&mut app);
        let clip = app.flow_canvas_clip().to_vec();
        let drawn: u32 = clip.iter().map(|rect| rect.area()).sum();
        assert!(drawn > 0 && drawn < canvas.area(), "{clip:?}");
        let menu = (canvas.top()..canvas.bottom())
            .flat_map(|y| (canvas.left()..canvas.right()).map(move |x| (x, y)))
            .find(|&(x, y)| buffer[(x, y)].symbol() != " ")
            .expect("the menu is over the canvas");
        assert!(
            !clip.iter().any(|rect| rect.contains(menu.into())),
            "{menu:?}"
        );
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
        assert!(app.flow_canvas().is_none());
    }
}
