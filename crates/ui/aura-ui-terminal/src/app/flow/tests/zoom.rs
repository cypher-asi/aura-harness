use super::*;

#[test]
fn zoom_keeps_the_point_under_the_pointer_where_it_is() {
    let mut app = hosted_app();
    assert_eq!(app.flow().zoom(), 1.0);
    app.zoom_flow(2.0, Some((20.0, 6.0)));
    assert_eq!(app.flow().zoom(), 2.0);
    assert_eq!(app.flow().pan(), (10.0, 3.0), "(20, 6) stays under it");

    // Zoomed in, the view shows less of the canvas, and pans by canvas
    // cells.
    let flow = app.flow_mut();
    assert_eq!(flow.sync_view(40.0, 10.0), (10.0, 3.0));
    flow.pan_by(-4.0, 0.0);
    assert_eq!(flow.pan(), (6.0, 3.0));

    // Out to the least, around the view's middle, which may take the view
    // off the canvas.
    app.zoom_flow(0.01, None);
    assert_eq!(app.flow().zoom(), MIN_ZOOM);
    assert_eq!(app.flow().pan(), (-64.0, -14.5));
    app.zoom_flow(100.0, None);
    assert_eq!(app.flow().zoom(), MAX_ZOOM);
}

#[test]
fn the_zoom_keys_work_only_when_a_front_end_draws_the_canvas() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Char('+'));
    app.zoom_flow(2.0, None);
    assert_eq!(app.flow().zoom(), 1.0, "the terminal does not zoom");

    let mut app = hosted_app();
    press(&mut app, KeyCode::Char('+'));
    assert_eq!(app.flow().zoom(), ZOOM_STEP);
    press(&mut app, KeyCode::Char('-'));
    press(&mut app, KeyCode::Char('-'));
    assert!((app.flow().zoom() - 1.0 / ZOOM_STEP).abs() < 1e-6);
    press(&mut app, KeyCode::Char('0'));
    assert!((app.flow().zoom() - 1.0).abs() < 1e-6);
}

#[test]
fn a_click_on_the_canvas_selects_a_line_then_opens_it_and_folds_at_the_marker() {
    let mut app = app_with_flow();
    let layout = app.flow().layout();
    let rect = layout.card_rect(1);
    let (x, y) = (cells(rect.x + 4) + 0.5, cells(rect.y + 2) + 0.5);
    assert!(app.click_flow_at(x, y));
    assert_eq!(selected(&app), "aa:3", "the row clicked");
    assert!(app.detail_view().is_none());
    assert!(app.click_flow_at(x, y));
    assert!(app.detail_view().is_some(), "the second click opens it");
    app.handle_key(KeyEvent::from(KeyCode::Esc));

    // The model call's marker folds the call; the turn's, the turn.
    let rect = app.flow().layout().card_rect(1);
    assert!(app.click_flow_at(cells(rect.x + 2) + 0.5, cells(rect.y) + 0.5));
    app.flow_mut().ensure_layout();
    assert!(app.flow().layout().cards[1].folded);
    assert!(!app.flow().layout().cards[0].folded);
    let rect = app.flow().layout().card_rect(0);
    assert!(app.click_flow_at(cells(rect.x + 2) + 0.5, cells(rect.y) + 0.5));
    app.flow_mut().ensure_layout();
    assert!(app.flow().layout().cards[0].folded);

    assert!(!app.click_flow_at(0.5, 0.5), "empty canvas");
}
