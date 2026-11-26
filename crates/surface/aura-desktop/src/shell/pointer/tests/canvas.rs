use super::*;

#[test]
fn the_wheel_scrolls_the_chat_by_pixels() {
    let mut stage = streamed_chat();
    let (width, height) = (stage.shell.window.width, stage.shell.window.height);
    stage
        .shell
        .pointer_moved(Point::new(width / 2.0, height / 2.0));
    stage.draw();

    // Moving over plain text changes nothing on screen.
    let cell = stage.shell.metrics.cell;
    stage.shell.pointer_moved(Point::new(
        width / 2.0 + 2.0 * cell.width,
        height / 2.0 + cell.height,
    ));
    assert!(
        !stage.shell.dirty(),
        "no frame for a move that shows nothing"
    );

    a_touchpad_slides_the_rows(&mut stage);
    let moved = a_notch_eases_out(&mut stage);
    a_spin_goes_further(&mut stage, moved);
    turning_back_goes_back_to_the_bottom(&mut stage);

    // Not over the tab bar.
    stage.shell.pointer_moved(Point::new(width / 2.0, 4.0));
    stage.draw();
    stage.shell.wheel(notch(30.0), stage.now);
    assert!(!stage.shell.dirty());
}

/// A chat streaming more lines than fit, drawn.
fn streamed_chat() -> Stage {
    let mut stage = stage();
    stage.seed(false);
    stage.shell.commands([UiCommand::StartStreaming]);
    let lines: String = (0..200).map(|n| format!("line {n}\n\n")).collect();
    stage.shell.commands([UiCommand::AppendText(lines)]);
    stage.draw();
    assert!(stage.shell.text().contains("line 199"));
    stage
}

fn offset(stage: &Stage) -> usize {
    stage.shell.tabs.front().tui.scroll_offset()
}

/// Draw frames until nothing moves, and say how many it took.
fn settle(stage: &mut Stage) -> i32 {
    let mut frames = 0;
    while stage.shell.dirty() && frames < 200 {
        stage.now += Duration::from_millis(16);
        stage.draw();
        frames += 1;
    }
    frames
}

fn notch(notches: f32) -> MouseScrollDelta {
    MouseScrollDelta::LineDelta(0.0, notches)
}

/// A touchpad: a third of a row up from the bottom is the row above
/// drawn two thirds of a row higher.
fn a_touchpad_slides_the_rows(stage: &mut Stage) {
    let cell = stage.shell.metrics.cell;
    let third = f64::from(cell.height / glide::TOUCHPAD_GAIN) / 3.0;
    let nudge = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, third));
    stage.shell.wheel(nudge, stage.now);
    assert!(stage.shell.dirty());
    assert_eq!(offset(stage), 1);
    stage.draw();

    // Another third slides the same rows.
    let rows = stage.shell.text();
    stage.shell.wheel(nudge, stage.now);
    assert!(stage.shell.dirty(), "the body slides");
    assert_eq!(offset(stage), 1);
    let slid = stage.pixels();
    assert_eq!(stage.shell.painter.built(), 0, "painted from kept rows");
    assert_eq!(stage.shell.text(), rows);
    assert!(slid == crate::shell::tests::afresh(&stage.shell));
}

/// A notch eases out over frames. Returns the rows four notches moved.
fn a_notch_eases_out(stage: &mut Stage) -> usize {
    let before = offset(stage);
    stage.now += Duration::from_secs(1);
    stage.shell.wheel(notch(4.0), stage.now);
    let frames = settle(stage);
    assert!((4..100).contains(&frames), "{frames} frames");
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rows = (4.0 * glide::NOTCH_ROWS) as usize;
    let moved = offset(stage) - before;
    assert!((rows - 1..=rows + 1).contains(&moved), "{moved} rows");
    assert!(!stage.shell.text().contains("line 199"), "scrolled up");
    assert!(stage.pixels() == crate::shell::tests::afresh(&stage.shell));
    moved
}

/// Notches of a spin go further each time than the `moved` rows of as
/// many notches apart.
fn a_spin_goes_further(stage: &mut Stage, moved: usize) {
    let before = offset(stage);
    stage.now += Duration::from_secs(1);
    for _ in 0..4 {
        stage.shell.wheel(notch(1.0), stage.now);
        stage.now += Duration::from_millis(30);
        stage.draw();
    }
    settle(stage);
    let spun = offset(stage) - before;
    assert!(
        spun > moved + 2,
        "a spin of 4 went {spun} rows, 4 notches {moved}"
    );
}

fn turning_back_goes_back_to_the_bottom(stage: &mut Stage) {
    // Turning back stops what is easing out.
    stage.now += Duration::from_secs(1);
    stage.shell.wheel(notch(4.0), stage.now);
    let before = offset(stage);
    stage.shell.wheel(notch(-1.0), stage.now);
    settle(stage);
    assert!(offset(stage) < before, "went back down");

    // Each turn is capped at `MOST_LINES`, so two get back from
    // however far the spin went.
    for _ in 0..2 {
        stage.shell.wheel(notch(-400.0), stage.now);
        settle(stage);
    }
    assert_eq!(offset(stage), 0, "and back to the bottom, on its rows");
    assert!(stage.shell.text().contains("line 199"));
    assert!(stage.pixels() == crate::shell::tests::afresh(&stage.shell));
}

/// The GRAPH tab shown and its view at the canvas's top left.
fn graph_at_home() -> Stage {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let graph = PaneTab::Chat(ChatTab::Graph);
    let tab = stage.shell.chrome.spot(graph).expect("the tab is drawn");
    stage.drag(tab, tab);
    crate::shell::snapshot::flow(&mut stage);
    stage
        .shell
        .key_pressed(&Press::of(NamedKey::Home), stage.now);
    stage.draw();
    stage
}

/// Canvas cell (x, y) on the grid's part of the window, where the
/// canvas is drawn now.
fn canvas_spots(stage: &Stage) -> impl Fn(f32, f32) -> Point {
    let area = stage.shell.tabs.front().tui.flow_canvas().expect("drawn");
    let origin = stage
        .shell
        .metrics
        .origin(stage.shell.grid_area(), stage.shell.cols);
    let cell = stage.shell.metrics.cell;
    move |x: f32, y: f32| {
        Point::new(
            origin.x + (f32::from(area.x) + x) * cell.width,
            origin.y + (f32::from(area.y) + y) * cell.height,
        )
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn the_graph_canvas_drags_by_the_pixel_and_clicks_its_cards() {
    let mut stage = graph_at_home();
    assert_eq!(pan(&stage), (0.0, 0.0));

    let at = canvas_spots(&stage);
    let cell = stage.shell.metrics.cell;
    let selected = |stage: &Stage| {
        let flow = stage.shell.tabs.front().tui.flow();
        flow.selected_node().map(|node| node.id.clone())
    };

    // The model call's head line, in the subcard under the turn's.
    let model = stage.shell.tabs.front().tui.flow().layout().card_rect(1);
    let head = at(model.x as f32 + 4.5, model.y as f32 + 1.5);
    stage.drag(head, head);
    assert_eq!(selected(&stage).as_deref(), Some("a1:2"));

    // Zoomed in, the canvas is wider than the view. Empty canvas
    // drags the view with the pointer, by the pixel.
    stage
        .shell
        .tabs
        .front_mut()
        .tui
        .zoom_flow(2.0, Some((0.0, 0.0)));
    stage.draw();
    let zoom = stage.shell.tabs.front().tui.flow().zoom();
    assert_eq!(zoom, 2.0);
    let bar = tab_bar::height(&stage.shell.metrics);
    let on_window = |at: Point| Point::new(at.x, at.y + bar);
    let from = at(6.5, 12.5);
    let before = pan(&stage);
    stage.shell.pointer_moved(on_window(from));
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default, "not yet");
    stage
        .shell
        .pointer_moved(on_window(Point::new(from.x - 30.0, from.y)));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Grabbing);
    stage.shell.button(MouseButton::Left, false, stage.now);
    stage.draw();
    let (x, y) = pan(&stage);
    let step = |pixels: f32| pixels / (cell.width * zoom);
    assert!((x - before.0 - step(30.0)).abs() < 1e-3, "{x}, {y}");
    assert_eq!(y, before.1);
    assert!(x.fract() != 0.0, "a fraction of a cell");
    assert_eq!(
        selected(&stage).as_deref(),
        Some("a1:2"),
        "the drag selects nothing"
    );
    assert!(stage.shell.take_effects().is_empty(), "nor any text");

    // A touchpad pans by what it says.
    let nudge = MouseScrollDelta::PixelDelta(PhysicalPosition::new(-12.0, 0.0));
    stage.shell.wheel(nudge, stage.now);
    stage.draw();
    assert!((pan(&stage).0 - before.0 - step(42.0)).abs() < 1e-3);
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn the_graph_canvas_zooms_around_the_pointer() {
    let mut stage = graph_at_home();

    let area = stage.shell.tabs.front().tui.flow_canvas().expect("drawn");
    let origin = stage
        .shell
        .metrics
        .origin(stage.shell.grid_area(), stage.shell.cols);
    let cell = stage.shell.metrics.cell;
    let bar = tab_bar::height(&stage.shell.metrics);
    let pointer = Point::new(
        origin.x + (f32::from(area.x) + 12.25) * cell.width,
        origin.y + (f32::from(area.y) + 6.5) * cell.height + bar,
    );
    stage.shell.pointer_moved(pointer);
    let under = |stage: &Stage| {
        let flow = stage.shell.tabs.front().tui.flow();
        let (pan, zoom) = (flow.pan(), flow.zoom());
        (pan.0 + 12.25 / zoom, pan.1 + 6.5 / zoom)
    };
    let before = under(&stage);

    // Ctrl and the wheel zoom in; the point under the pointer stays.
    stage.shell.modifiers = ModifiersState::CONTROL;
    stage
        .shell
        .wheel(MouseScrollDelta::LineDelta(0.0, 3.0), stage.now);
    stage.draw();
    let zoom = stage.shell.tabs.front().tui.flow().zoom();
    assert!((zoom - 1.1f32.powi(3)).abs() < 1e-4, "{zoom}");
    let after = under(&stage);
    assert!(
        (after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3,
        "{before:?} {after:?}"
    );

    // A pinch zooms too, no further out than the least, and around
    // the pointer though that takes the view off the canvas.
    stage.shell.modifiers = ModifiersState::empty();
    for _ in 0..10 {
        stage.shell.pinch(-0.5);
    }
    stage.draw();
    assert_eq!(stage.shell.tabs.front().tui.flow().zoom(), FLOW_MIN_ZOOM);
    let after = under(&stage);
    assert!(
        (after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3,
        "{before:?} {after:?}"
    );
    assert!(pan(&stage).0 < 0.0, "{:?}", pan(&stage));

    // Without Ctrl, the wheel pans.
    stage
        .shell
        .wheel(MouseScrollDelta::LineDelta(0.0, 1.0), stage.now);
    assert_eq!(stage.shell.tabs.front().tui.flow().zoom(), FLOW_MIN_ZOOM);
}

fn pan(stage: &Stage) -> (f32, f32) {
    stage.shell.tabs.front().tui.flow().pan()
}
