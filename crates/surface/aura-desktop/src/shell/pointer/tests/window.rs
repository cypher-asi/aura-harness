use super::*;

#[test]
fn the_window_edges_resize_it() {
    use ResizeDirection::{East, North, NorthWest, SouthEast, SouthWest};
    let size = Size::new(800.0, 600.0);
    let at = |x, y| edge(size, Point::new(x, y), 1.0);
    assert_eq!(at(400.0, 2.0), Some(North));
    assert_eq!(at(798.0, 300.0), Some(East));
    assert_eq!(at(2.0, 2.0), Some(NorthWest));
    assert_eq!(at(10.0, 2.0), Some(NorthWest), "near the corner");
    assert_eq!(at(2.0, 590.0), Some(SouthWest));
    assert_eq!(at(799.0, 599.0), Some(SouthEast));
    assert_eq!(at(400.0, 300.0), None);
    assert_eq!(at(400.0, 10.0), None, "the tab bar below the band");
    // Twice the pixels, twice the band.
    assert_eq!(edge(size, Point::new(400.0, 10.0), 2.0), Some(North));
}

#[test]
fn the_edges_resize_the_window_and_show_it() {
    let mut stage = stage();
    stage.draw();
    stage
        .shell
        .pointer_moved(Point::new(WINDOW.width - 2.0, 400.0));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::EwResize);
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(
        stage.shell.take_effects(),
        [Effect::Resize(ResizeDirection::East)]
    );
    stage
        .shell
        .pointer_moved(Point::new(3.0, WINDOW.height - 3.0));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::NeswResize);
    stage.shell.pointer_moved(Point::new(400.0, 400.0));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);
    stage.shell.pointer_left();
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);
}

#[test]
fn the_tab_bar_moves_and_maximizes_the_window() {
    let mut stage = stage();
    stage.draw();
    // Between the title and the window boxes.
    let boxes = stage.shell.bar.spot(Hit::Minimize).expect("the boxes");
    let empty = Point::new(boxes.x - 60.0, boxes.y);
    stage.shell.pointer_moved(empty);
    assert_eq!(stage.shell.bar.hover(), None, "nothing is there");
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(stage.shell.take_effects(), [Effect::Drag]);

    stage.now += Duration::from_millis(200);
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(stage.shell.take_effects(), [Effect::ToggleMaximize]);
    // The third press of three is the first of the next pair.
    stage.now += Duration::from_millis(200);
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(stage.shell.take_effects(), [Effect::Drag]);
    stage.now += Duration::from_secs(1);
    stage.shell.button(MouseButton::Left, true, stage.now);
    assert_eq!(stage.shell.take_effects(), [Effect::Drag], "too late");

    stage.click(Hit::Minimize, MouseButton::Left);
    assert_eq!(stage.shell.take_effects(), [Effect::Minimize]);
    stage.click(Hit::Maximize, MouseButton::Left);
    assert_eq!(stage.shell.take_effects(), [Effect::ToggleMaximize]);
    stage.click(Hit::Close, MouseButton::Left);
    assert_eq!(stage.shell.take_effects(), [Effect::Exit]);
}

#[test]
fn hovering_a_box_lights_it_and_asks_for_a_frame() {
    let mut stage = stage();
    let plain = stage.pixels();
    let at = stage.shell.bar.spot(Hit::Close).expect("the box");
    stage.shell.pointer_moved(at);
    assert_eq!(stage.shell.bar.hover(), Some(Hit::Close));
    assert!(stage.shell.dirty());
    let lit = stage.pixels();
    assert!(plain != lit);

    stage.shell.pointer_moved(Point::new(at.x + 1.0, at.y));
    assert!(!stage.shell.dirty(), "still on it: nothing new to draw");
    stage.shell.pointer_left();
    assert!(stage.shell.dirty());
    assert!(plain == stage.pixels());
}

#[test]
fn the_hamburger_box_under_the_close_box_hides_and_shows_the_right_column() {
    let mut stage = stage();
    stage.draw();
    let close = stage.shell.bar.spot(Hit::Close).expect("the close box");
    let rect = stage
        .shell
        .chrome
        .side_toggle_box()
        .expect("the hamburger box");
    let bar = tab_bar::height(&stage.shell.metrics);
    let middle = Point::new(rect.center_x(), rect.center_y() + bar);
    assert!((middle.x - close.x).abs() <= 1.0, "{middle:?} {close:?}");
    assert!(rect.y + bar > close.y, "below it");

    stage.shell.pointer_moved(middle);
    assert_eq!(stage.shell.chrome.hover(), Some(chrome::Hit::SideToggle));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
    assert!(stage.shell.tabs.front().tui.side_column_shown());
    stage.shell.button(MouseButton::Left, true, stage.now);
    stage.shell.button(MouseButton::Left, false, stage.now);
    stage.draw();
    assert!(!stage.shell.tabs.front().tui.side_column_shown());
    assert_eq!(
        stage.shell.chrome.side_toggle_box(),
        Some(rect),
        "it stays put"
    );

    stage.shell.pointer_moved(middle);
    stage.shell.button(MouseButton::Left, true, stage.now);
    stage.shell.button(MouseButton::Left, false, stage.now);
    assert!(stage.shell.tabs.front().tui.side_column_shown());
}

#[test]
fn tabs_open_switch_and_close_from_the_bar() {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let _ = stage.asked();

    stage.click(Hit::New, MouseButton::Left);
    assert!(stage.asked().is_empty(), "nothing until a folder is picked");
    stage.pick_folder();
    assert_eq!(stage.shell.tabs.len(), 1, "the tab opens with its project");
    stage.created("a3", "agent03");
    assert_eq!(stage.shell.tabs.len(), 2);
    assert_eq!(stage.shell.tabs.front_index(), 1);
    let _ = stage.asked();

    stage.click(Hit::Tab(0), MouseButton::Left);
    assert_eq!(stage.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(stage.shell.tabs.front_index(), 0);

    stage.click(Hit::Tab(1), MouseButton::Middle);
    assert_eq!(stage.shell.tabs.len(), 1);
    assert!(stage.shell.take_effects().is_empty());
}

#[test]
fn panel_tabs_and_dividers_belong_to_the_window() {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    assert!(
        !stage.shell.text().contains("TERMINAL"),
        "tab labels are drawn, not cells"
    );

    let terminal = PaneTab::Chat(ChatTab::Terminal);
    let tab = stage.shell.chrome.spot(terminal).expect("the tab is drawn");
    stage.shell.pointer_moved(Point::new(
        tab.x,
        tab.y + tab_bar::height(&stage.shell.metrics),
    ));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
    stage.drag(tab, tab);
    assert_eq!(stage.shell.tabs.front().tui.chat_tab(), ChatTab::Terminal);
    assert!(stage.chat().focused);
    assert_eq!(stage.chat().active, Some(terminal));

    let before = stage.chat().area;
    let band = stage
        .shell
        .chrome
        .band(Divider::Agents)
        .expect("agents beside the chat");
    let cell = stage.shell.metrics.cell.width;
    stage.shell.pointer_moved(Point::new(
        band.x,
        band.y + tab_bar::height(&stage.shell.metrics),
    ));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::EwResize);
    stage.drag(band, Point::new(band.x + 10.0 * cell, band.y));
    let after = stage.chat().area;
    assert_eq!(after.right(), before.right());
    assert_eq!(after.x, before.x + 10, "{before:?} → {after:?}");
}
