use super::*;
use crate::app::ChatTab;
use crate::events::{FlowKind, FlowLane, FlowNode, TraceNodeStatus, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
    app_with_flow_and(Vec::new())
}

/// [`app_with_flow`], with `more` nodes after it.
fn app_with_flow_and(more: Vec<FlowNode>) -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    let mut model = node(LEAD, 2, 105, FlowKind::Model, "sonnet");
    model.detail = "1.2k/300 · 4s".into();
    let mut read = node(LEAD, 3, 108, FlowKind::Tool, "read_file src/lib.rs");
    read.detail = "12ms".into();
    let mut call = node(LEAD, 4, 110, FlowKind::Tool, "task coder · write greet()");
    call.links = vec![
        ("spawn".into(), "bb:1".into()),
        ("result".into(), "aa:5".into()),
    ];
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
                ..FlowLane::default()
            },
            FlowLane {
                agent_id: CODER.into(),
                name: "coder #1".into(),
                parent: Some(LEAD.into()),
                ..FlowLane::default()
            },
        ],
        nodes: [
            node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
            model,
            read,
            call,
            node(CODER, 1, 120, FlowKind::Prompt, "write greet()"),
            reply,
            node(LEAD, 5, 140, FlowKind::Result, "coder #1 replied"),
            pending,
        ]
        .into_iter()
        .chain(more)
        .collect(),
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
fn a_turn_is_a_main_card_with_its_model_calls_on_a_spine() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    let rows = screen(&mut app, 84, 24);
    let expected = [
        "│                                                                                  │",
        "│  ┌ ▾ ✓ turn ───────────── 2 steps ┐                                              │",
        "│  │ add a greeting                 │                                              │",
        "│  └─┬──────────────────────────────┘                                              │",
        "│    │                                                                             │",
        "│    │ ┌ ▾ ✓ model ── 1.2k/300 · 4s ┐                                              │",
        "│    ├─┤ sonnet                     │                                              │",
        "│    │ │ ✓ read_file src/lib.… 12ms │                                              │",
        "│    │ │ ✓ task coder · write… 4.2s ├────┐                                         │",
        "│    │ └────────────────────────────┘    │                                         │",
        "│    │                                   │                                         │",
        "│    │ ┌ ● model ───────────────────┐    │                                         │",
        "│    └─┤ sonnet                     │    │                                         │",
        "│      └────────────────────────────┘    │                                         │",
        "│                                        │                                         │",
        "│                                        │ ▾ coder #1                              │",
        "│                                        │ ┌ ✓ turn ────────────── coder #1 ┐      │",
        "│                                        └─┤ write greet()                  │      │",
        "│                                          │ ↳ done                         │      │",
        "│                                          └─────────── ↩ reported to Lead ─┘      │",
    ];
    assert_eq!(rows[1..=20], expected, "\n{}", rows.join("\n"));
}

#[test]
fn turns_go_down_a_column_while_it_has_room_then_wrap_right() {
    let more = || {
        vec![
            node(LEAD, 7, 160, FlowKind::Prompt, "now say bye"),
            node(LEAD, 8, 165, FlowKind::Model, "haiku"),
        ]
    };
    let mut app = app_with_flow_and(more());
    let rows = screen(&mut app, 84, 40);
    assert!(
        rows.iter().any(|row| row.starts_with("│  │ now say bye")),
        "under the first turn, folded now\n{}",
        rows.join("\n")
    );

    let mut app = app_with_flow_and(more());
    press(&mut app, KeyCode::Home);
    let rows = screen(&mut app, 84, 12);
    assert!(
        rows.iter()
            .any(|row| row.contains("add a greeting") && row.contains("now say bye")),
        "no room under it: the next column\n{}",
        rows.join("\n")
    );
}

#[test]
fn a_folded_turn_is_its_main_card_and_says_how_much_it_hides() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char(' '));
    let rows = screen(&mut app, 120, 16).join("\n");
    assert!(
        rows.contains("┌ ▸ ✓ turn ──────────── +2 steps ┐"),
        "{rows}"
    );
    assert!(!rows.contains("sonnet"), "{rows}");
    assert!(
        rows.contains("add a greeting                 ├────┐"),
        "the spawn comes from the main card\n{rows}"
    );
    assert!(rows.contains("└─┤ write greet()"), "{rows}");
}

#[test]
fn a_folded_model_call_hides_its_tool_calls() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");
    press(&mut app, KeyCode::Char(' '));
    let rows = screen(&mut app, 120, 24).join("\n");
    assert!(rows.contains("┌ ▸ ✓ model ─ +2 tools · "), "{rows}");
    assert!(!rows.contains("read_file"), "{rows}");
    assert!(
        rows.contains("├─┤ sonnet                     ├────┐"),
        "the spawn comes from the model call\n{rows}"
    );
    press(&mut app, KeyCode::Char(' '));
    let rows = screen(&mut app, 120, 24).join("\n");
    assert!(rows.contains("read_file"), "{rows}");
}

#[test]
fn the_view_follows_the_selection_across_the_canvas() {
    let mut app = app_with_flow();
    let rows = screen(&mut app, 80, 10).join("\n");
    assert!(rows.contains("● model"), "the newest card, pending\n{rows}");
    assert!(!rows.contains("add a greeting"), "{rows}");
    press(&mut app, KeyCode::Home);
    let rows = screen(&mut app, 80, 10).join("\n");
    assert!(rows.contains("add a greeting"), "{rows}");

    // Panned. Other agents are named over their cards; the agent on
    // screen goes unnamed.
    app.pan_flow(40.0, 14.0);
    let rows = screen(&mut app, 80, 10).join("\n");
    assert!(!rows.contains("add a greeting"));
    assert!(rows.contains("coder #1"), "{rows}");
    assert!(
        rows.lines()
            .all(|row| !row.contains("Lead") || row.contains("↩ reported to Lead")),
        "only the report back names the agent on screen\n{rows}"
    );
}

#[test]
fn clicks_select_and_open_lines_and_drags_and_the_wheel_pan() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    let buffer = whole_screen(&mut app);
    let label = find(&buffer, "sonnet").unwrap();
    let marker = find(&buffer, "▾ ✓ turn").unwrap();
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

    // A drag on empty canvas moves the view with the pointer, over a
    // flow wider than the screen.
    let turns = (0..12).flat_map(|turn| {
        let at = 200 + turn * 10;
        [
            node(LEAD, 7 + 2 * turn, at, FlowKind::Prompt, "and then"),
            node(LEAD, 8 + 2 * turn, at + 5, FlowKind::Model, "haiku"),
        ]
    });
    let mut app = app_with_flow_and(turns.collect());
    press(&mut app, KeyCode::Home);
    let buffer = whole_screen(&mut app);
    // The gutter right of the prompt.
    let (x, y) = find(&buffer, "add a greeting").unwrap();
    let empty = (x + 35, y);
    let pan = app.flow().pan();
    mouse(&mut app, down, empty);
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        (empty.0 - 5, y),
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (empty.0 - 5, y),
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
#[allow(clippy::cast_precision_loss)]
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
    let model = app.flow().layout().card_rect(1);
    assert!(app.click_flow_at(model.x as f32 + 4.5, model.y as f32 + 1.5));
    assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");

    // Away from the tab, there is no canvas to draw.
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Terminal);
    let _ = whole_screen(&mut app);
    assert!(app.flow_canvas().is_none());
    assert!(app.flow_canvas_clip().is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));

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
fn a_canvas_larger_than_the_view_has_a_map_that_moves_the_view() {
    let mut small = app_with_flow();
    let _ = screen(&mut small, 160, 60);
    assert_eq!(small.flow().minimap(), None, "it all fits");

    let turns = (0..12).flat_map(|turn| {
        let at = 200 + turn * 10;
        [
            node(LEAD, 7 + 2 * turn, at, FlowKind::Prompt, "and then"),
            node(LEAD, 8 + 2 * turn, at + 5, FlowKind::Model, "haiku"),
        ]
    });
    let mut app = app_with_flow_and(turns.collect());
    press(&mut app, KeyCode::Home);
    let buffer = whole_screen(&mut app);
    let map = app.flow().minimap().expect("the map is drawn");
    assert!(find(&buffer, " map ").is_some());
    let inner = map.inner();
    let cells = || {
        (inner.top()..inner.bottom())
            .flat_map(|y| (inner.left()..inner.right()).map(move |x| (x, y)))
    };
    let theme = Theme::cyber();
    assert!(
        cells().any(|at| buffer[at].bg == theme.colors.selection),
        "the view is lit on it"
    );
    assert!(cells().any(|at| buffer[at].symbol() != " "), "cards on it");

    // A press on its far end moves the view there; the selection stays.
    let pan = app.flow().pan();
    let far = (inner.right() - 1, inner.y);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), far);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), far);
    assert!(app.flow().pan().0 > pan.0, "{:?}", app.flow().pan());
    assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");

    press(&mut app, KeyCode::Char('m'));
    let buffer = whole_screen(&mut app);
    assert!(find(&buffer, " map ").is_none(), "m hides it");
    assert_eq!(app.flow().minimap(), None);
    press(&mut app, KeyCode::Char('m'));

    // Over a canvas the front end draws, the map's cells are left to
    // the terminal.
    app.set_host_canvas(true);
    let _ = whole_screen(&mut app);
    let map = app.flow().minimap().expect("the map is drawn");
    assert!(!app.flow_canvas_clip().is_empty());
    assert!(
        app.flow_canvas_clip()
            .iter()
            .all(|rect| rect.intersection(map.area).is_empty()),
        "{:?} {:?}",
        app.flow_canvas_clip(),
        map.area
    );
}

#[test]
fn says_so_while_loading_and_when_nothing_is_recorded() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
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
