//! The graph of a flow: on the canvas, expanded, zoomed and under a menu.

use super::fixtures::seed;
use super::flows::{benchmark_flow, flow};
use super::support::snapshot;
use crate::geom::Point;
use crate::shell::keys::Press;
use crate::shell::tests::{stage, Stage};
use aura_ui_terminal::{ChatTab, Pane, PaneTab};
use ratatui::layout::Rect;
use winit::keyboard::NamedKey;

/// Press the chat's expand box.
fn expand(h: &mut Stage) {
    let at = h.shell.chrome.expand_box(Pane::Chat).expect("drawn");
    let at = Point::new(at.center_x(), at.center_y());
    h.drag(at, at);
}

/// Expanded, the graph has the whole window; pressed again, it is back.
fn expanded_and_back(h: &mut Stage, canvas: Rect) {
    expand(h);
    let tui = &h.shell.tabs.front().tui;
    assert!(tui.chat_expanded());
    let wide = tui.flow_canvas().expect("the canvas is on screen");
    assert!(wide.width > canvas.width, "{wide:?} over {canvas:?}");
    snapshot("graph-expanded", h);
    expand(h);
    assert!(!h.shell.tabs.front().tui.chat_expanded());
}

fn delegation_selected(h: &mut Stage) {
    h.shell.key_pressed(&Press::of(NamedKey::Home), h.now);
    for _ in 0..3 {
        h.shell.key_pressed(&Press::of(NamedKey::ArrowDown), h.now);
    }
    let tui = &h.shell.tabs.front().tui;
    assert_eq!(
        tui.flow().selected_node().map(|n| n.id.as_str()),
        Some("a1:4")
    );
    snapshot("graph-delegation", h);
}

/// Zoomed out, the whole flow fits; further out, text is bars.
fn zoomed_out(h: &mut Stage) {
    h.shell.tabs.front_mut().tui.zoom_flow(0.5, None);
    snapshot("graph-zoomed-out", h);
    h.shell.tabs.front_mut().tui.zoom_flow(0.5, None);
    h.shell.frame(h.now);
    h.shell.tabs.front_mut().tui.zoom_flow(4.0, None);
}

/// A menu over the canvas stays over it.
fn menu_over_canvas(h: &mut Stage, canvas: Rect) {
    h.type_text("/");
    h.shell.frame(h.now);
    let tui = &h.shell.tabs.front().tui;
    let clip = tui.flow_canvas_clip();
    let drawn: u32 = clip.iter().map(|rect| rect.area()).sum();
    assert!(drawn > 0 && drawn < canvas.area(), "{clip:?}");
    snapshot("graph-menu", h);
}

#[test]
fn graph_canvas() {
    let mut h = stage();
    seed(&mut h);
    let graph = PaneTab::Chat(ChatTab::Graph);
    let tab = h.shell.chrome.spot(graph).expect("the tab is drawn");
    h.drag(tab, tab);
    flow(&mut h);
    let screen = h.screen();
    assert!(
        !screen.contains("qwen3-coder"),
        "the window draws the cards\n{screen}"
    );
    let tui = &h.shell.tabs.front().tui;
    let canvas = tui.flow_canvas().expect("the canvas is on screen");
    assert_eq!(tui.flow_canvas_clip(), [canvas]);
    snapshot("graph", &mut h);

    expanded_and_back(&mut h, canvas);
    delegation_selected(&mut h);
    zoomed_out(&mut h);
    menu_over_canvas(&mut h, canvas);
}

#[test]
fn graph_of_a_benchmark() {
    let mut h = stage();
    seed(&mut h);
    let graph = PaneTab::Chat(ChatTab::Graph);
    let tab = h.shell.chrome.spot(graph).expect("the tab is drawn");
    h.drag(tab, tab);
    benchmark_flow(&mut h);
    h.shell.frame(h.now);
    let tui = &h.shell.tabs.front().tui;
    assert!(tui.flow().is_big());
    assert!(tui.flow().minimap().is_some(), "the map is drawn");
    snapshot("graph-benchmark", &mut h);

    let time = PaneTab::Chat(ChatTab::Time);
    let tab = h.shell.chrome.spot(time).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.chat_tab(), ChatTab::Time);
    snapshot("time-benchmark", &mut h);
}
