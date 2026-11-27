use super::support::record_over_the_chat;
use super::*;

#[test]
fn the_chats_scrollbar_scrolls_it_and_the_divider_still_moves_beside_it() {
    let mut stage = long_chat();
    let chat = stage.chat().area;
    let (cell, area) = (stage.shell.metrics.cell, stage.shell.grid_area());
    let origin = stage.shell.metrics.origin(area, stage.shell.cols);
    let at = |col: u16, row: u16| {
        Point::new(
            origin.x + (f32::from(col) + 0.5) * cell.width,
            origin.y + (f32::from(row) + 0.5) * cell.height,
        )
    };
    let border = chat.right() - 1;
    let middle = at(border, chat.y + chat.height / 2);
    let bar = tab_bar::height(&stage.shell.metrics);
    stage
        .shell
        .pointer_moved(Point::new(middle.x, middle.y + bar));
    assert_eq!(stage.shell.chrome.hover(), None, "no divider on the border");
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);

    assert_eq!(stage.shell.tabs.front().tui.scroll_offset(), 0);
    stage.drag(middle, at(border, chat.y));
    let tui = &stage.shell.tabs.front().tui;
    assert!(tui.scroll_offset() > 0, "the chat scrolled up");
    assert!(tui.selection().is_none(), "nothing is selected");
    assert_eq!(stage.chat().area, chat, "the divider stayed");
    assert!(stage.shell.take_effects().is_empty(), "nothing copied");

    // The blank columns left of the border take hold of it too, two
    // of them, so it is wider to hit than the line it draws.
    let beside = border - 2;
    let from = at(beside, chat.y + chat.height / 2);
    stage.shell.pointer_moved(Point::new(from.x, from.y + bar));
    assert_eq!(stage.shell.chrome.hover(), None);
    stage.drag(from, at(beside, chat.bottom() - 1));
    let tui = &stage.shell.tabs.front().tui;
    assert_eq!(tui.scroll_offset(), 0, "back at the bottom");
    assert!(tui.selection().is_none(), "nothing is selected");
    assert!(stage.shell.take_effects().is_empty(), "nothing copied");

    let band = stage
        .shell
        .chrome
        .band(Divider::Right)
        .expect("beside the chat");
    stage.drag(band, Point::new(band.x - 10.0 * cell.width, band.y));
    let after = stage.chat().area;
    assert_eq!(after.x, chat.x);
    assert_eq!(after.right(), chat.right() - 10, "{chat:?} → {after:?}");
}

/// A chat with more replies than fit, drawn.
fn long_chat() -> Stage {
    use aura_ui::events::{MessageData, MessageRole};

    let mut stage = stage();
    stage.seed(false);
    stage.shell.commands((0..80).map(|i| {
        UiCommand::ShowMessage(MessageData::new(
            MessageRole::Assistant,
            format!("reply {i}"),
        ))
    }));
    stage.draw();
    stage
}

#[test]
fn the_close_box_closes_what_covers_the_chat() {
    let mut stage = record_over_the_chat();
    assert!(stage.chat().closable, "the record over the chat closes");

    let close = stage
        .shell
        .chrome
        .close_box(Pane::Chat)
        .expect("the close box is drawn");
    let close = Point::new(close.center_x(), close.center_y());
    stage.shell.pointer_moved(Point::new(
        close.x,
        close.y + tab_bar::height(&stage.shell.metrics),
    ));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
    stage.drag(close, close);
    assert!(stage.shell.tabs.front().tui.detail_view().is_none());
    assert!(!stage.chat().closable);
    assert!(stage.chat().focused);
}

#[test]
fn the_expand_box_fills_the_window_with_the_chat_and_restores_it() {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    assert_eq!(stage.chat().expand, Some(false));
    let panes = stage.shell.tabs.front().tui.pane_chrome().len();
    assert!(panes > 1, "the other panels are beside the chat");

    let press = |stage: &mut Stage| {
        let expand = stage
            .shell
            .chrome
            .expand_box(Pane::Chat)
            .expect("the expand box is drawn");
        let middle = Point::new(expand.center_x(), expand.center_y());
        stage.shell.pointer_moved(Point::new(
            middle.x,
            middle.y + tab_bar::height(&stage.shell.metrics),
        ));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
        stage.drag(middle, middle);
        stage.draw();
    };
    press(&mut stage);
    let tui = &stage.shell.tabs.front().tui;
    assert!(tui.chat_expanded());
    assert_eq!(tui.pane_chrome().len(), 1, "only the chat is left");
    assert_eq!(stage.chat().expand, Some(true));

    press(&mut stage);
    let tui = &stage.shell.tabs.front().tui;
    assert!(!tui.chat_expanded());
    assert_eq!(tui.pane_chrome().len(), panes);
}

#[test]
fn the_star_box_switches_the_side_panels_lists_to_the_project() {
    use aura_ui_terminal::SideScope;
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let side = |stage: &Stage| {
        let tui = &stage.shell.tabs.front().tui;
        let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Side);
        pane.expect("the side panel has a bar").clone()
    };
    assert_eq!(side(&stage).scope, Some(false));

    let all = stage
        .shell
        .chrome
        .scope_box(Pane::Side)
        .expect("the * box is drawn");
    let middle = Point::new(all.center_x(), all.center_y());
    stage.shell.pointer_moved(Point::new(
        middle.x,
        middle.y + tab_bar::height(&stage.shell.metrics),
    ));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
    stage.drag(middle, middle);
    let tui = &stage.shell.tabs.front().tui;
    assert_eq!(tui.side_scope(), SideScope::Project);
    assert_eq!(side(&stage).scope, Some(true));
    assert!(side(&stage).focused);
}

#[test]
fn the_side_expand_box_maximizes_the_side_panel_over_the_records() {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let has = |stage: &Stage, pane: Pane| {
        let tui = &stage.shell.tabs.front().tui;
        tui.pane_chrome().iter().any(|c| c.pane == pane)
    };
    assert!(has(&stage, Pane::Records));
    let press = |stage: &mut Stage| {
        let expand = stage
            .shell
            .chrome
            .expand_box(Pane::Side)
            .expect("the side panel's expand box is drawn");
        let middle = Point::new(expand.center_x(), expand.center_y());
        stage.drag(middle, middle);
        stage.draw();
    };
    press(&mut stage);
    let tui = &stage.shell.tabs.front().tui;
    assert!(tui.side_expanded());
    assert!(!has(&stage, Pane::Records), "the side panel covers it");

    press(&mut stage);
    assert!(!stage.shell.tabs.front().tui.side_expanded());
    assert!(has(&stage, Pane::Records));
}

#[test]
fn the_cross_sits_in_the_middle_of_the_close_box() {
    let mut stage = record_over_the_chat();
    // Odd and even boxes, thin and thick strokes.
    for _ in 0..12 {
        cross_is_centered(&mut stage);
        stage.key(ModifiersState::CONTROL, Press::typing("="));
        stage.draw();
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn cross_is_centered(stage: &mut Stage) {
    let mut close = stage.shell.chrome.close_box(Pane::Chat).expect("drawn");
    close.y += tab_bar::height(&stage.shell.metrics);
    let pixels = stage.pixels();
    let width = WINDOW.width as usize;
    let at = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        pixels[i..i + 3].iter().map(|&c| f32::from(c)).sum::<f32>()
    };
    // Inside the outline, which is a few pixels thick at most.
    let inset = 4.0;
    let xs = (close.x + inset) as usize..(close.x + close.width - inset) as usize;
    let ys = (close.y + inset) as usize..(close.y + close.height - inset) as usize;
    let ground = at(xs.start, ys.start);
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in ys.clone() {
        for x in xs.clone() {
            let ink = (at(x, y) - ground).abs();
            sum += ink;
            sx += ink * (x as f32 + 0.5);
            sy += ink * (y as f32 + 0.5);
        }
    }
    assert!(sum > 0.0, "the cross is drawn");
    let (cx, cy) = (sx / sum, sy / sum);
    let size = stage.shell.font_size;
    assert!(
        (cx - close.center_x()).abs() < 0.25,
        "{cx} in {close:?} at {size}"
    );
    assert!(
        (cy - close.center_y()).abs() < 0.25,
        "{cy} in {close:?} at {size}"
    );
}

#[test]
fn the_pointer_is_a_hand_over_a_link_in_the_chat() {
    use aura_ui::events::{MessageData, MessageRole};

    let mut stage = stage();
    stage.seed(false);
    stage
        .shell
        .commands([UiCommand::ShowMessage(MessageData::new(
            MessageRole::Assistant,
            "Read https://example.com/docs first".to_string(),
        ))]);
    stage.draw();
    let (cell, area) = (stage.shell.metrics.cell, stage.shell.grid_area());
    let origin = stage.shell.metrics.origin(area, stage.shell.cols);
    let bar = tab_bar::height(&stage.shell.metrics);
    let at = |col: u16, row: u16| {
        Point::new(
            origin.x + (f32::from(col) + 0.5) * cell.width,
            origin.y + (f32::from(row) + 0.5) * cell.height + bar,
        )
    };
    let cells: Vec<(u16, u16)> = stage
        .shell
        .tabs
        .front()
        .tui
        .prose_rows()
        .iter()
        .flat_map(|row| (row.x..row.x + row.width).map(move |x| (x, row.y)))
        .collect();
    let mut over_link = None;
    let mut over_text = None;
    for (x, y) in cells {
        stage.shell.pointer_moved(at(x, y));
        if stage.shell.tabs.front().tui.hovers_link() {
            over_link.get_or_insert((x, y));
        } else {
            over_text.get_or_insert((x, y));
        }
    }
    let (x, y) = over_link.expect("the link is under the pointer somewhere");
    stage.shell.pointer_moved(at(x, y));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
    let (x, y) = over_text.expect("the words around it are not a link");
    stage.shell.pointer_moved(at(x, y));
    assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);
}
